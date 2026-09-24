//! Traceability index and phase-gate evidence (PRD 17.5, TR-1).
//!
//! Phase-gate evidence is generated from the traceability index: requirement →
//! implementation component → verification artifact → last passing version →
//! known exception/waiver (PRD 17.5). A CI check fails on gaps (TR-1). Waivers are
//! immutable, scoped, expiring artifacts naming the requirement, rationale,
//! compensating control, approver and review date; a waiver never converts a
//! failed hard safety/authority invariant into a passing proof (PRD 17.5).

use serde::{Deserialize, Serialize};

use crate::{Phase, Requirement, Verification, REGISTRY};

/// A verification record linking a requirement to the artifact that verifies it
/// and the last version at which it passed (PRD 17.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerificationRecord {
    /// Requirement id (e.g. `RS-1`).
    pub requirement_id: String,
    /// The verification artifact (test name, drill, approval reference).
    pub artifact: String,
    /// The last version at which this verification passed.
    pub last_passing_version: String,
}

/// A scoped, expiring waiver (PRD 17.5). Immutable once issued.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Waiver {
    /// Requirement id the waiver covers.
    pub requirement_id: String,
    /// Why the requirement is temporarily unmet.
    pub rationale: String,
    /// The compensating control in place meanwhile.
    pub compensating_control: String,
    /// The approving principal.
    pub approver: String,
    /// Expiry / review timestamp (unix seconds).
    pub expires_at: u64,
}

impl Waiver {
    /// Whether the waiver is still active at `now`.
    pub fn is_active(&self, now: u64) -> bool {
        now < self.expires_at
    }
}

/// Whether a requirement may be waived at all. A waiver never converts a failed
/// hard safety/authority *proof* into a passing one (PRD 17.5), so requirements
/// verified by static proof are non-waivable.
pub fn is_waivable(req: &Requirement) -> bool {
    req.verification != Verification::StaticProof
}

/// A gap in phase-gate evidence: a required requirement with no passing
/// verification and no active waiver (PRD 17.5, TR-1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gap {
    /// The uncovered requirement id.
    pub requirement_id: String,
    /// Why it is a gap.
    pub reason: GapReason,
}

/// Why a requirement is a phase-gate gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GapReason {
    /// No verification record exists.
    Unverified,
    /// A waiver exists but has expired.
    WaiverExpired,
}

/// The traceability index (PRD 17.5).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TraceabilityIndex {
    /// Verification records.
    pub records: Vec<VerificationRecord>,
    /// Waivers.
    pub waivers: Vec<Waiver>,
}

impl TraceabilityIndex {
    /// Create an empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a verification record.
    pub fn verify(
        &mut self,
        requirement_id: impl Into<String>,
        artifact: impl Into<String>,
        version: impl Into<String>,
    ) {
        self.records.push(VerificationRecord {
            requirement_id: requirement_id.into(),
            artifact: artifact.into(),
            last_passing_version: version.into(),
        });
    }

    /// Add a waiver.
    pub fn waive(&mut self, waiver: Waiver) {
        self.waivers.push(waiver);
    }

    fn is_verified(&self, id: &str) -> bool {
        self.records.iter().any(|r| r.requirement_id == id)
    }

    fn active_waiver(&self, id: &str, now: u64) -> Option<&Waiver> {
        self.waivers
            .iter()
            .find(|w| w.requirement_id == id && w.is_active(now))
    }

    /// Compute phase-gate gaps for a phase (PRD 17.5, TR-1): every cross-cutting
    /// requirement first enforced at or before `phase` that has no passing
    /// verification and no active, permissible waiver. CI fails when this is
    /// non-empty.
    pub fn phase_gate_gaps(&self, phase: Phase, now: u64) -> Vec<Gap> {
        REGISTRY
            .iter()
            .filter(|r| r.first_enforced <= phase)
            .filter_map(|r| {
                if self.is_verified(r.id) {
                    return None;
                }
                match self.active_waiver(r.id, now) {
                    // An active, permissible waiver covers a non-proof requirement.
                    Some(_) if is_waivable(r) => None,
                    Some(_) => Some(Gap {
                        requirement_id: r.id.into(),
                        reason: GapReason::Unverified,
                    }),
                    None => {
                        // Distinguish an expired waiver from never-waived.
                        let expired = self.waivers.iter().any(|w| w.requirement_id == r.id);
                        Some(Gap {
                            requirement_id: r.id.into(),
                            reason: if expired {
                                GapReason::WaiverExpired
                            } else {
                                GapReason::Unverified
                            },
                        })
                    }
                }
            })
            .collect()
    }

    /// Whether the phase gate passes (no gaps) — the TR-1 CI condition.
    pub fn phase_gate_passes(&self, phase: Phase, now: u64) -> bool {
        self.phase_gate_gaps(phase, now).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_verification_is_a_gap() {
        let index = TraceabilityIndex::new();
        // Nothing verified -> every Phase 0 requirement is a gap.
        let gaps = index.phase_gate_gaps(Phase::P0, 100);
        assert!(!gaps.is_empty());
        assert!(gaps.iter().all(|g| g.reason == GapReason::Unverified));
    }

    #[test]
    fn verifying_all_phase0_requirements_passes_the_gate() {
        let mut index = TraceabilityIndex::new();
        for r in REGISTRY.iter().filter(|r| r.first_enforced <= Phase::P0) {
            index.verify(r.id, format!("test::{}", r.id), "0.0.0");
        }
        assert!(index.phase_gate_passes(Phase::P0, 100));
    }

    #[test]
    fn active_waiver_covers_a_gap_but_expired_does_not() {
        // Pick a waivable Phase 0 requirement.
        let target = REGISTRY
            .iter()
            .find(|r| r.first_enforced <= Phase::P0 && is_waivable(r))
            .unwrap();
        // Verify everything else so only `target` could be a gap.
        let mut index = TraceabilityIndex::new();
        for r in REGISTRY
            .iter()
            .filter(|r| r.first_enforced <= Phase::P0 && r.id != target.id)
        {
            index.verify(r.id, "t", "0.0.0");
        }
        index.waive(Waiver {
            requirement_id: target.id.into(),
            rationale: "pending Phase 1 harness".into(),
            compensating_control: "manual review".into(),
            approver: "risk-owner".into(),
            expires_at: 1_000,
        });
        // Active waiver -> gate passes.
        assert!(index.phase_gate_passes(Phase::P0, 500));
        // Expired waiver -> gap reappears.
        let gaps = index.phase_gate_gaps(Phase::P0, 2_000);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].reason, GapReason::WaiverExpired);
    }
}
