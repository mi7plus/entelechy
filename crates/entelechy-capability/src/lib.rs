//! Capability discovery and the CapabilityGraph.
//!
//! PRD v11 references: section 13.1 (capability discovery), 5.2 (CapabilityGraph),
//! 7.6 / CD-7 (untrusted metadata), Q13 (unattested MCP defaults & attestation
//! expiry).
//!
//! Effect classes, reversibility and scope declared by MCP servers or OpenAPI
//! documents are untrusted hints; authoritative classification comes from operator
//! attestation or sandbox-verified probing, and capabilities with unknown effects
//! default to external and irreversible (CD-7). An unattested MCP server receives
//! no write authority and needs human approval for any call at L3 or above (Q13).
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use entelechy_ir::{AttestationLevel, EffectClass, EffectMetadata};

/// How long an operator attestation remains valid (PRD Q13: 90 days).
pub const ATTESTATION_TTL_SECS: u64 = 90 * 24 * 60 * 60;

/// The kind of source a capability was discovered from (PRD CD-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    /// An MCP server.
    Mcp,
    /// An OpenAPI service.
    OpenApi,
    /// A database.
    Database,
    /// A file store.
    FileStore,
    /// A supplied corpus.
    Corpus,
    /// A native / in-process tool.
    Native,
}

/// A discovered capability (PRD CD-2, 5.2 CapabilityGraph invariant: every
/// capability has schema, effect class, scope, reliability and provenance).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    /// Capability name.
    pub name: String,
    /// Where it came from.
    pub source: SourceKind,
    /// JSON schema of its inputs/outputs.
    pub schema: serde_json::Value,
    /// The *declared* effect metadata (an untrusted hint until attested — CD-7).
    pub declared_effect: EffectMetadata,
    /// Authority scopes it requires.
    pub authority_scope: Vec<String>,
    /// Observed latency in milliseconds, if profiled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// Observed per-call cost in minor currency units, if profiled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_minor: Option<u64>,
    /// Observed reliability in `[0,1]`, if profiled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reliability: Option<f64>,
    /// When this capability was operator-attested (unix seconds), if ever (Q13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attested_at: Option<u64>,
    /// The version string attested; attestation expires on any version change (Q13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attested_version: Option<String>,
    /// The current advertised version (compared against `attested_version`).
    pub version: String,
}

impl Capability {
    /// Whether a valid operator attestation is in force at `now` (Q13): attested,
    /// not expired (90-day TTL) and the attested version still matches.
    pub fn is_attested_at(&self, now: u64) -> bool {
        // Sandbox-verified declarations are authoritative without an operator TTL.
        if self.declared_effect.attestation == AttestationLevel::SandboxVerified {
            return true;
        }
        match (self.attested_at, &self.attested_version) {
            (Some(at), Some(ver)) => {
                self.declared_effect.attestation == AttestationLevel::OperatorAttested
                    && *ver == self.version
                    && now.saturating_sub(at) <= ATTESTATION_TTL_SECS
            }
            _ => false,
        }
    }

    /// The *effective* effect metadata to enforce at `now` (CD-7, Q13). When not
    /// attested, effects are treated conservatively as external + irreversible and
    /// write authority is withheld.
    pub fn effective_effect(&self, now: u64) -> EffectMetadata {
        if self.is_attested_at(now) {
            self.declared_effect.clone()
        } else {
            EffectMetadata::conservative_default(&self.declared_effect.operation_key_namespace)
        }
    }

    /// Whether this capability may be granted write authority at `now` (Q13:
    /// an unattested server receives no write authority).
    pub fn may_have_write_authority(&self, now: u64) -> bool {
        let eff = self.effective_effect(now);
        self.is_attested_at(now) && matches!(eff.class, EffectClass::Read | EffectClass::Write)
    }

    /// Whether any call requires human approval at L3+ (Q13: unattested servers do).
    pub fn requires_human_approval_at_shadow(&self, now: u64) -> bool {
        !self.is_attested_at(now)
    }
}

/// The typed capability inventory (PRD 5.2).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CapabilityGraph {
    /// Capabilities by name.
    capabilities: BTreeMap<String, Capability>,
}

impl CapabilityGraph {
    /// Schema id for the artifact (PRD 5.9).
    pub const SCHEMA_ID: &'static str = "entelechy.capabilitygraph";

    /// Empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a capability.
    pub fn add(&mut self, capability: Capability) {
        self.capabilities.insert(capability.name.clone(), capability);
    }

    /// Look up a capability.
    pub fn get(&self, name: &str) -> Option<&Capability> {
        self.capabilities.get(name)
    }

    /// Iterate capabilities.
    pub fn iter(&self) -> impl Iterator<Item = &Capability> {
        self.capabilities.values()
    }

    /// Compare required capability names to the graph and emit gaps (CD-4). A gap
    /// is a missing capability or one present but not usable for a required write.
    pub fn gaps(&self, required: &[RequiredCapability], now: u64) -> Vec<CapabilityGap> {
        required
            .iter()
            .filter_map(|req| match self.get(&req.name) {
                None => Some(CapabilityGap {
                    name: req.name.clone(),
                    reason: GapReason::Missing,
                }),
                Some(cap) if req.needs_write && !cap.may_have_write_authority(now) => {
                    Some(CapabilityGap {
                        name: req.name.clone(),
                        reason: GapReason::NoWriteAuthority,
                    })
                }
                Some(_) => None,
            })
            .collect()
    }

    /// Detect capability drift against a newer graph (CD-6): additions, removals
    /// and changed declared effect classes.
    pub fn drift_since(&self, newer: &CapabilityGraph) -> Vec<DriftEvent> {
        let mut events = Vec::new();
        for (name, old) in &self.capabilities {
            match newer.get(name) {
                None => events.push(DriftEvent {
                    name: name.clone(),
                    kind: DriftKind::Removed,
                }),
                Some(new) if new.declared_effect.class != old.declared_effect.class => {
                    events.push(DriftEvent {
                        name: name.clone(),
                        kind: DriftKind::EffectChanged,
                    });
                }
                Some(new) if new.version != old.version => events.push(DriftEvent {
                    name: name.clone(),
                    kind: DriftKind::VersionChanged,
                }),
                _ => {}
            }
        }
        for name in newer.capabilities.keys() {
            if !self.capabilities.contains_key(name) {
                events.push(DriftEvent {
                    name: name.clone(),
                    kind: DriftKind::Added,
                });
            }
        }
        events
    }
}

/// A capability the GoalSpec requires (PRD CD-4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequiredCapability {
    /// Required capability name.
    pub name: String,
    /// Whether the objective needs write authority for it.
    pub needs_write: bool,
}

/// A capability gap (PRD CD-4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapabilityGap {
    /// The required capability name.
    pub name: String,
    /// Why it is a gap.
    pub reason: GapReason,
}

/// Why a required capability is a gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GapReason {
    /// No such capability was discovered.
    Missing,
    /// Present, but write authority cannot be granted (unattested — Q13).
    NoWriteAuthority,
}

/// A capability drift event (PRD CD-6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DriftEvent {
    /// The capability name.
    pub name: String,
    /// The kind of drift.
    pub kind: DriftKind,
}

/// The kind of capability drift (PRD CD-6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DriftKind {
    /// A new capability appeared.
    Added,
    /// A capability disappeared.
    Removed,
    /// A capability's declared effect class changed.
    EffectChanged,
    /// A capability's version changed (invalidates attestation — Q13).
    VersionChanged,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mcp_write(name: &str, attested: bool, now_attested_at: u64) -> Capability {
        Capability {
            name: name.into(),
            source: SourceKind::Mcp,
            schema: serde_json::json!({}),
            declared_effect: EffectMetadata {
                class: EffectClass::Write,
                idempotent: true,
                reversible: true,
                dry_run_supported: true,
                read_back_supported: true,
                attestation: if attested {
                    AttestationLevel::OperatorAttested
                } else {
                    AttestationLevel::UntrustedHint
                },
                operation_key_namespace: name.into(),
            },
            authority_scope: vec![],
            latency_ms: None,
            cost_minor: None,
            reliability: None,
            attested_at: if attested { Some(now_attested_at) } else { None },
            attested_version: if attested { Some("v1".into()) } else { None },
            version: "v1".into(),
        }
    }

    #[test]
    fn unattested_effects_default_conservative_and_no_write() {
        let cap = mcp_write("create_ticket", false, 0);
        let now = 1_000_000;
        assert_eq!(cap.effective_effect(now).class, EffectClass::Irreversible);
        assert!(!cap.may_have_write_authority(now));
        assert!(cap.requires_human_approval_at_shadow(now));
    }

    #[test]
    fn fresh_operator_attestation_grants_write() {
        let now = 1_000_000;
        let cap = mcp_write("create_ticket", true, now);
        assert!(cap.is_attested_at(now));
        assert!(cap.may_have_write_authority(now));
        assert_eq!(cap.effective_effect(now).class, EffectClass::Write);
    }

    #[test]
    fn attestation_expires_after_ttl_and_on_version_change() {
        let attested_at = 1_000_000;
        let cap = mcp_write("create_ticket", true, attested_at);
        // Just after the 90-day TTL.
        assert!(!cap.is_attested_at(attested_at + ATTESTATION_TTL_SECS + 1));
        // Version bump invalidates.
        let mut bumped = cap.clone();
        bumped.version = "v2".into();
        assert!(!bumped.is_attested_at(attested_at + 10));
    }

    #[test]
    fn gaps_report_missing_and_unusable() {
        let now = 1_000_000;
        let mut g = CapabilityGraph::new();
        g.add(mcp_write("create_ticket", false, 0)); // unattested → no write
        let req = vec![
            RequiredCapability { name: "create_ticket".into(), needs_write: true },
            RequiredCapability { name: "issue_refund".into(), needs_write: true },
        ];
        let gaps = g.gaps(&req, now);
        assert_eq!(gaps.len(), 2);
        assert!(gaps.iter().any(|x| x.name == "create_ticket" && x.reason == GapReason::NoWriteAuthority));
        assert!(gaps.iter().any(|x| x.name == "issue_refund" && x.reason == GapReason::Missing));
    }

    #[test]
    fn drift_detects_changes() {
        let mut old = CapabilityGraph::new();
        old.add(mcp_write("a", true, 0));
        old.add(mcp_write("b", true, 0));
        let mut new = CapabilityGraph::new();
        let mut a2 = mcp_write("a", true, 0);
        a2.version = "v2".into();
        new.add(a2);
        new.add(mcp_write("c", true, 0));
        let events = old.drift_since(&new);
        assert!(events.iter().any(|e| e.name == "a" && e.kind == DriftKind::VersionChanged));
        assert!(events.iter().any(|e| e.name == "b" && e.kind == DriftKind::Removed));
        assert!(events.iter().any(|e| e.name == "c" && e.kind == DriftKind::Added));
    }
}
