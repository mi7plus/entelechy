//! Assurance compiler and SafetyCase (PRD 14.1, 5.2).
//!
//! PRD v11 references: section 14.1 (assurance compiler), 5.2 (SafetyCase), 5.6
//! (constraint classes & claim semantics), 7.4 (static IR validation), 9.2
//! (feasibility per class).
//!
//! A SafetyCase maps claims to evidence; each claim cites static checks,
//! policies, tests or approvals (PRD 5.2). Feasibility is evaluated per class
//! (PRD 5.6): structural claims need a passing proof, behavioral claims need an
//! upper-bound test, operational claims need runtime caps. A failed structural
//! proof makes the design infeasible — it is never a scalar penalty (PRD 9.2).
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

use entelechy_eval::ConstraintClass;
use entelechy_ir::{validate, CapabilityCatalog, Program, Violation};

/// A piece of evidence supporting a claim (PRD 14.1 assurance inputs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Evidence {
    /// A passing static IR proof of an invariant (e.g. `IR-I2`, `IR-I3`).
    StaticProof {
        /// The invariant id proven.
        invariant: String,
    },
    /// A passing behavioral upper-bound test: violation-rate upper bound ≤ epsilon
    /// with confidence 1-delta over N trials (PRD 5.6).
    UpperBoundTest {
        /// Upper bound on the violation rate.
        upper_bound: f64,
        /// The epsilon it must not exceed.
        epsilon: f64,
        /// Number of independent trials.
        n: usize,
    },
    /// An operational cap enforced at runtime (PRD 5.6 operational).
    RuntimeCap {
        /// What is capped (e.g. `p95_latency_ms`).
        metric: String,
    },
    /// A coarse holdout gate result (EV-14).
    HoldoutGate {
        /// Whether the gate passed.
        passed: bool,
    },
    /// A signed approval bound to artifact hashes (PRD 14.5).
    Approval {
        /// The approval's key id (for provenance).
        key_id: String,
    },
    /// Supply-chain evidence for generated/native components (PRD 14.1).
    SupplyChain {
        /// A short descriptor (e.g. `sbom`, `cargo-audit`).
        descriptor: String,
    },
}

/// A claim in the SafetyCase (PRD 5.2). Its class determines what evidence
/// suffices (PRD 5.6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Stable claim id.
    pub id: String,
    /// Human-readable statement (e.g. "the system never issues a refund").
    pub statement: String,
    /// Constraint class (PRD 5.6).
    pub class: ConstraintClass,
    /// Cited evidence.
    pub evidence: Vec<Evidence>,
}

/// Whether a claim is adequately supported for its class (PRD 5.6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClaimStatus {
    /// Structural claim with a passing proof: proven for the released IR/policy.
    Proven,
    /// Behavioral claim with a passing upper-bound test.
    StatisticallySupported,
    /// Operational claim with a runtime cap.
    OperationallyBounded,
    /// Not enough evidence of the right kind (PRD 5.6).
    Insufficient {
        /// Why the evidence is insufficient.
        reason: String,
    },
}

impl Claim {
    /// Evaluate whether this claim is supported for its class (PRD 5.6).
    pub fn status(&self) -> ClaimStatus {
        match self.class {
            ConstraintClass::Structural => {
                if self.evidence.iter().any(|e| matches!(e, Evidence::StaticProof { .. })) {
                    ClaimStatus::Proven
                } else {
                    ClaimStatus::Insufficient {
                        reason: "structural claim needs a static proof (PRD 5.6 Rule 1)".into(),
                    }
                }
            }
            ConstraintClass::Behavioral => {
                match self
                    .evidence
                    .iter()
                    .find_map(|e| match e {
                        Evidence::UpperBoundTest { upper_bound, epsilon, .. } => Some((*upper_bound, *epsilon)),
                        _ => None,
                    }) {
                    Some((ub, eps)) if ub <= eps => ClaimStatus::StatisticallySupported,
                    Some(_) => ClaimStatus::Insufficient {
                        reason: "upper bound exceeds epsilon (PRD 5.6 Rule 2)".into(),
                    },
                    None => ClaimStatus::Insufficient {
                        reason: "behavioral claim needs an upper-bound test".into(),
                    },
                }
            }
            ConstraintClass::Operational => {
                if self.evidence.iter().any(|e| matches!(e, Evidence::RuntimeCap { .. })) {
                    ClaimStatus::OperationallyBounded
                } else {
                    ClaimStatus::Insufficient {
                        reason: "operational claim needs a runtime cap".into(),
                    }
                }
            }
        }
    }

    /// Whether this claim is supported.
    pub fn is_supported(&self) -> bool {
        !matches!(self.status(), ClaimStatus::Insufficient { .. })
    }
}

/// A SafetyCase: claims mapped to evidence plus known limitations (PRD 5.2).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SafetyCase {
    /// The claims.
    pub claims: Vec<Claim>,
    /// Known limitations (PRD 14.1 SafetyCase records known limitations).
    pub known_limitations: Vec<String>,
}

impl SafetyCase {
    /// Schema id for the artifact (PRD 5.9).
    pub const SCHEMA_ID: &'static str = "entelechy.safetycase";

    /// Claims that are not adequately supported.
    pub fn unsupported(&self) -> Vec<&Claim> {
        self.claims.iter().filter(|c| !c.is_supported()).collect()
    }

    /// Share of negative-goal-style claims enforced structurally rather than only
    /// behaviorally (PRD success metric "structural enforcement share").
    pub fn structural_enforcement_share(&self) -> f64 {
        let relevant: Vec<&Claim> = self
            .claims
            .iter()
            .filter(|c| matches!(c.class, ConstraintClass::Structural | ConstraintClass::Behavioral))
            .collect();
        if relevant.is_empty() {
            return 1.0;
        }
        let structural = relevant.iter().filter(|c| c.class == ConstraintClass::Structural).count();
        structural as f64 / relevant.len() as f64
    }
}

/// The output of the assurance compiler (PRD 14.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssuranceReport {
    /// Static IR validation violations (PRD 7.4). Empty means the structural
    /// invariants hold.
    pub static_violations: Vec<StaticViolation>,
    /// The SafetyCase.
    pub safety_case: SafetyCase,
    /// The policy snapshot version in force (PRD 5.2).
    pub policy_snapshot_version: u32,
}

/// A serializable view of an IR invariant violation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StaticViolation {
    /// Invariant id.
    pub code: String,
    /// Node id.
    pub node_id: String,
    /// Message.
    pub message: String,
}

impl From<&Violation> for StaticViolation {
    fn from(v: &Violation) -> Self {
        Self {
            code: v.code.to_string(),
            node_id: v.node_id.clone(),
            message: v.message.clone(),
        }
    }
}

impl AssuranceReport {
    /// Whether the design is release-eligible: no unresolved structural
    /// violations and every SafetyCase claim supported (PRD 14.1, 9.2).
    pub fn release_eligible(&self) -> bool {
        self.static_violations.is_empty() && self.safety_case.unsupported().is_empty()
    }
}

/// The assurance compiler (PRD 14.1). Runs static IR validation and assembles the
/// report around a provided SafetyCase.
pub struct AssuranceCompiler;

impl AssuranceCompiler {
    /// Compile assurance for a program: static validation (PRD 7.4) plus the
    /// SafetyCase and policy snapshot version.
    pub fn compile<C: CapabilityCatalog>(
        program: &Program,
        catalog: &C,
        safety_case: SafetyCase,
        policy_snapshot_version: u32,
    ) -> AssuranceReport {
        let static_violations = validate(program, catalog)
            .iter()
            .map(StaticViolation::from)
            .collect();
        AssuranceReport {
            static_violations,
            safety_case,
            policy_snapshot_version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn structural_claim(supported: bool) -> Claim {
        Claim {
            id: "C-refund".into(),
            statement: "the system never issues a refund".into(),
            class: ConstraintClass::Structural,
            evidence: if supported {
                vec![Evidence::StaticProof { invariant: "IR-I2".into() }]
            } else {
                vec![]
            },
        }
    }

    #[test]
    fn structural_claim_needs_a_proof() {
        assert_eq!(structural_claim(true).status(), ClaimStatus::Proven);
        assert!(matches!(
            structural_claim(false).status(),
            ClaimStatus::Insufficient { .. }
        ));
    }

    #[test]
    fn behavioral_claim_needs_upper_bound_within_epsilon() {
        let ok = Claim {
            id: "C-disclosure".into(),
            statement: "no cross-customer disclosure".into(),
            class: ConstraintClass::Behavioral,
            evidence: vec![Evidence::UpperBoundTest { upper_bound: 0.008, epsilon: 0.01, n: 300 }],
        };
        assert_eq!(ok.status(), ClaimStatus::StatisticallySupported);
        let bad = Claim {
            class: ConstraintClass::Behavioral,
            evidence: vec![Evidence::UpperBoundTest { upper_bound: 0.03, epsilon: 0.01, n: 100 }],
            ..ok.clone()
        };
        assert!(matches!(bad.status(), ClaimStatus::Insufficient { .. }));
    }

    #[test]
    fn structural_enforcement_share() {
        let case = SafetyCase {
            claims: vec![
                structural_claim(true),
                Claim {
                    id: "C-b".into(),
                    statement: "b".into(),
                    class: ConstraintClass::Behavioral,
                    evidence: vec![Evidence::UpperBoundTest { upper_bound: 0.0, epsilon: 0.01, n: 300 }],
                },
            ],
            known_limitations: vec![],
        };
        assert!((case.structural_enforcement_share() - 0.5).abs() < 1e-9);
        assert!(case.unsupported().is_empty());
    }
}
