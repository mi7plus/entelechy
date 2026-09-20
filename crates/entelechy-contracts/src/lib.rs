//! Normative requirement registry and traceability (PRD 17.5, 17.6).
//!
//! This PRD is normative where it uses requirement IDs, invariants, release
//! gates, phase exits or MUST/SHALL language (PRD 17.5). Every normative
//! requirement has a stable identifier, a verification owner, and a phase in
//! which it is first enforced. Phase-gate evidence is generated from this index:
//! requirement -> implementation component -> verification artifact (PRD 17.5).
//!
//! This crate seeds the cross-cutting registry from PRD section 17.6 and provides
//! the types a CI traceability check (TR-1) consumes.
#![forbid(unsafe_code)]

pub mod traceability;

use serde::{Deserialize, Serialize};

pub use traceability::{Gap, GapReason, TraceabilityIndex, VerificationRecord, Waiver};

/// A lifecycle phase (PRD section 21).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Phase {
    /// Phase 0 — Prove the thesis.
    P0,
    /// Phase 1 — Foundations.
    P1,
    /// Phase 2 — Objective to optimized single agent.
    P2,
    /// Phase 3 — Evidence-driven architecture synthesis.
    P3,
    /// Phase 4 — Assurance, release and operations.
    P4,
    /// Phase 5 — Scale and ecosystem.
    P5,
}

/// The verification owner of a requirement (PRD 17.5: each requirement has at
/// least one verification owner).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verification {
    /// Static proof / compiler check.
    StaticProof,
    /// Unit or property test.
    UnitTest,
    /// Conformance / integration test.
    Conformance,
    /// Integration test.
    Integration,
    /// Study evidence.
    StudyEvidence,
    /// Operational drill.
    OperationalDrill,
    /// Human approval.
    HumanApproval,
    /// CI check.
    CiCheck,
    /// Load test.
    LoadTest,
    /// Restore drill.
    RestoreDrill,
    /// Key-management drill.
    KeyDrill,
}

/// A normative requirement record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Requirement {
    /// Stable identifier, e.g. `"IR-I3"`, `"AC-1"`.
    pub id: &'static str,
    /// PRD section(s) that define it.
    pub section: &'static str,
    /// One-line summary.
    pub summary: &'static str,
    /// Its verification owner (PRD 17.5).
    pub verification: Verification,
    /// Phase in which it is first enforced (PRD 17.6).
    pub first_enforced: Phase,
}

/// The cross-cutting requirement registry from PRD 17.6.
///
/// Family requirements (OC, IR-I, RK, EV, MK, CD, TS, OP) remain authoritative in
/// their own sections and are added here as those crates land.
pub const REGISTRY: &[Requirement] = &[
    req("AC-1", "5.9", "Every artifact carries a schema id and version; readers fail closed on unsupported versions", Verification::Conformance, Phase::P0),
    req("AC-2", "5.9, Q22", "Canonical serialization and digest agility; artifact identity stable across releases", Verification::UnitTest, Phase::P1),
    req("AC-3", "5.9, 14.5", "Migrations deterministic; approvals survive only registered semantics-preserving migrations", Verification::Conformance, Phase::P1),
    req("PV-1", "5.10", "Every model call records provider identity fields, endpoint, parameters and returned revision", Verification::Integration, Phase::P0),
    req("PV-2", "5.10, 21.1.1", "ModelFingerprint and evidence epochs; no silent pooling across material drift", Verification::StudyEvidence, Phase::P0),
    req("PV-3", "5.10", "Drift beyond tolerance blocks automatic promotion", Verification::OperationalDrill, Phase::P4),
    req("RS-1", "8.6", "Runs and nodes end in typed terminal states; unknown commits enter reconciliation", Verification::Conformance, Phase::P0),
    req("RS-2", "8.6", "A human timeout is never an implicit approval", Verification::UnitTest, Phase::P1),
    req("EL-1", "9.6", "Holdout gate returns coarse responses, audits every query and enforces budgets", Verification::Conformance, Phase::P2),
    req("EL-2", "21.1.1", "Holdout manifest sealed before optimization; published holdout becomes regression evidence only", Verification::StudyEvidence, Phase::P0),
    req("RB-1", "14.6", "Automatic rollback triggers declared; stop authority separate from promote authority", Verification::OperationalDrill, Phase::P4),
    req("RB-2", "14.6", "Every L4/L5 deployment has a tested rollback target or approved no-rollback rationale", Verification::OperationalDrill, Phase::P4),
    req("PC-1", "16.6", "Protocol version negotiation; unsupported combinations fail closed with a machine-readable error", Verification::Conformance, Phase::P1),
    req("PC-2", "16.6", "No silent downgrade of security-relevant guarantees", Verification::Conformance, Phase::P4),
    req("PC-3", "16.6", "Plugin manifests declare compatibility; loading fails closed", Verification::Conformance, Phase::P5),
    req("DR-1", "17.3", "Backup and restore preserve hashes, lineage and authorization metadata", Verification::RestoreDrill, Phase::P1),
    req("DR-2", "17.3", "Erasure and revocation survive restore; post-restore effects are reconciled first", Verification::RestoreDrill, Phase::P4),
    req("DR-3", "17.3", "Partial loss marks evidence_incomplete and blocks new promotion", Verification::Integration, Phase::P4),
    req("OL-1", "17.4", "Quotas and bounded retries are enforced outside model output", Verification::LoadTest, Phase::P1),
    req("OL-2", "17.4", "Fallbacks independently authorized; degraded mode journaled and excluded from unregistered evidence", Verification::LoadTest, Phase::P4),
    req("DP-1", "17.1", "An installation declares one deployment profile; guarantees apply only within it", Verification::HumanApproval, Phase::P1),
    req("AU-1", "17.2", "Privileged server operations carry authenticated identity and separate authorization; expiry fails closed", Verification::Integration, Phase::P4),
    req("AU-2", "17.2", "Key rotation and revocation without plaintext artifact mutation; revoked keys never used for new writes", Verification::KeyDrill, Phase::P4),
    req("AU-3", "17.2", "Security-sensitive time decisions use an authenticated/monotonic source and fail closed on uncertainty", Verification::Integration, Phase::P4),
    req("SL-1", "18.1", "Every L5 SLO states window, population, statistic/error budget and exclusions; correctness gates sit outside error budgets", Verification::HumanApproval, Phase::P4),
    req("TR-1", "17.5", "Traceability index links requirements to owners, verification artifacts and waivers; CI fails on gaps", Verification::CiCheck, Phase::P1),
    req("DX-1", "16.5", "A new user reaches the first study report in under 30 minutes", Verification::CiCheck, Phase::P2),
];

const fn req(
    id: &'static str,
    section: &'static str,
    summary: &'static str,
    verification: Verification,
    first_enforced: Phase,
) -> Requirement {
    Requirement {
        id,
        section,
        summary,
        verification,
        first_enforced,
    }
}

/// Look up a requirement by its stable ID.
pub fn find(id: &str) -> Option<&'static Requirement> {
    REGISTRY.iter().find(|r| r.id == id)
}

/// All requirements first enforced at or before `phase`.
pub fn required_by(phase: Phase) -> impl Iterator<Item = &'static Requirement> {
    REGISTRY.iter().filter(move |r| r.first_enforced <= phase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_the_27_cross_cutting_ids() {
        // PRD 17.6 / Appendix J: "Registry of 27 IDs".
        assert_eq!(REGISTRY.len(), 27);
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<&str> = REGISTRY.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate requirement IDs in registry");
    }

    #[test]
    fn phase0_subset_is_nonempty_and_lookup_works() {
        assert!(required_by(Phase::P0).count() > 0);
        assert_eq!(find("IR-does-not-exist"), None);
        assert_eq!(find("AC-1").unwrap().first_enforced, Phase::P0);
    }
}
