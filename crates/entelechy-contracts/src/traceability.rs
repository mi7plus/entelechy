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

/// The lifecycle phase whose gate CI currently enforces (PRD §21). The gate is at
/// the Phase-5 exit: every cross-cutting requirement first enforced at or before P5
/// has an automated verification record in [`phase_gate_evidence`]. P5 adds exactly
/// one registry requirement, PC-3 (plugin-manifest compatibility), whose fail-closed
/// loading is verified in `entelechy-protocol` and enforced on the real plugin
/// loader in `entelechy-gateway`. P5 is the final roadmap phase, so this is the
/// furthest the requirements gate advances.
pub const CURRENT_GATE_PHASE: Phase = Phase::P5;

/// Default review horizon for a pre-maturity waiver (2027-07-01T00:00:00Z): past
/// this instant such a waiver expires and the TR-1 gate reports it as a gap,
/// forcing a re-review (PRD 17.5 — waivers are expiring). The current evidence set
/// has no active waivers — every requirement through the current gate is verified —
/// but the constant is retained for any future scoped waiver and by the gate tests.
pub const WAIVER_REVIEW_DEADLINE: u64 = 1_814_400_000;

/// The authoritative phase-gate evidence for [`CURRENT_GATE_PHASE`] (PRD 17.5,
/// TR-1): a verification record for every cross-cutting requirement enforced
/// through the current gate (now P5). Every such requirement has automated evidence
/// — real executed code with its own tests, operational drills and cross-crate
/// integration tests — so no waivers remain, though the waiver mechanism stays
/// available for future scoped, expiring gaps. The P4 advance added records for the
/// seven P2/P4 requirements PV-3, EL-1, PC-2, DR-2, DR-3, OL-2 and DX-1 (RB-1/RB-2,
/// AU-1/AU-2/AU-3 and SL-1, also P4, were recorded earlier). The P5 advance adds the
/// one P5 requirement, PC-3 (plugin-manifest compatibility, fail-closed loading).
///
/// `entelechy requirements --check` and the `traceability` CI job consume this. A
/// requirement first enforced at or before [`CURRENT_GATE_PHASE`] that is neither
/// recorded nor covered by an active, permissible waiver fails the gate.
pub fn phase_gate_evidence() -> TraceabilityIndex {
    // Pre-1.0 workspace version; the gate pins evidence to the build that produced it.
    const V: &str = "0.0.0";
    let mut index = TraceabilityIndex::new();

    // Requirements backed by automated verification artifacts.
    index.verify(
        "AC-1",
        "entelechy-artifacts: artifact_id_includes_header + tests/golden_format.rs",
        V,
    );
    index.verify(
        "AC-2",
        "entelechy-artifacts: tests/golden_format.rs + tests/proptest_canonical.rs",
        V,
    );
    index.verify(
        "AC-3",
        "entelechy-release: ApprovalBinding tests (stale_approval_blocks_even_with_override)",
        V,
    );
    index.verify(
        "PV-1",
        "entelechy-runtime: tests/golden_journal.rs + entelechy-gateway model identity capture",
        V,
    );
    index.verify(
        "RS-1",
        "entelechy-effects: non_idempotent_unknown_commit_needs_reconciliation + entelechy-runtime terminal-state tests",
        V,
    );
    index.verify(
        "RS-2",
        "entelechy-runtime: human_node_fails_closed_and_never_approves",
        V,
    );
    index.verify(
        "EL-2",
        "entelechy-eval: seal_rejects_non_holdout_tasks + entelechy-bench manifest_seals_holdout_hashes",
        V,
    );
    index.verify(
        "PC-1",
        "entelechy-protocol: disjoint_ranges_fail_closed + no_silent_security_downgrade",
        V,
    );
    index.verify(
        "OL-1",
        "entelechy-runtime: model_budget_exhaustion_is_typed_terminal (budgets enforced outside model output)",
        V,
    );
    index.verify(
        "TR-1",
        "CI job `traceability` (entelechy requirements --check) + entelechy-contracts phase_gate_evidence tests",
        V,
    );
    index.verify(
        "DR-1",
        "entelechy-artifacts: backup::backup_restore_preserves_hashes_lineage_and_authorization \
         (+ a_corrupt_backup_is_rejected_not_loaded, restore_re_enforces_the_tenant_firewall, \
         a_restore_never_resurrects_erased_data); operator drill `entelechy drill --kind restore`",
        V,
    );
    // RB-1/RB-2 are first enforced at P4; their operational-drill evidence is
    // recorded now so the P4 gate is already covered when the gate advances.
    index.verify(
        "RB-1",
        "entelechy-release: drill::rollback_drill_satisfies_rb1_and_rb2 + stop_authority_is_separate_from_promote \
         (declared auto-triggers fire; stop authority separate from promote); operator drill `entelechy drill --kind rollback`",
        V,
    );
    index.verify(
        "RB-2",
        "entelechy-release: drill::rollback_drill_satisfies_rb1_and_rb2 + l5_requires_rollback_ready \
         (production promotion refused without a tested rollback target or approved rationale); operator drill `entelechy drill --kind rollback`",
        V,
    );
    // AU-1/AU-2/AU-3 are first enforced at P4; their audit/attestation operational
    // and key-drill evidence is recorded now so the P4 gate is already covered when
    // the gate advances (mirrors RB-1/RB-2).
    index.verify(
        "AU-1",
        "entelechy-identity: drill::audit_drill_satisfies_au1_au2_au3 + credential_expiry_fails_closed \
         + audit::{valid_chain_verifies, external_anchor_detects_full_rewrite} (authenticated identity, \
         separate authorization, fail-closed expiry, tamper-evident externally-anchored log); operator \
         drill `entelechy drill --kind audit`",
        V,
    );
    index.verify(
        "AU-2",
        "entelechy-identity: drill::audit_drill_satisfies_au1_au2_au3 + keyring::revoked_key_cannot_sign \
         + expired_and_revoked_and_uncertain_fail_closed (key rotation/revocation with a stable content \
         address; a revoked key never signs new writes and its approval fails closed); operator drill \
         `entelechy drill --kind audit`",
        V,
    );
    index.verify(
        "AU-3",
        "entelechy-identity: drill::audit_drill_satisfies_au1_au2_au3 + expired_and_revoked_and_uncertain_fail_closed \
         (a security-sensitive time decision on an uncertain clock fails closed); operator drill \
         `entelechy drill --kind audit`",
        V,
    );
    // SL-1 is first enforced at P4; its SLO operational-drill evidence is recorded now
    // so the P4 gate is already covered when the gate advances (mirrors RB/AU).
    index.verify(
        "SL-1",
        "entelechy-release: slo::slo_drill_satisfies_sl1 + an_l5_slo_must_state_all_four \
         + error_budget_is_one_minus_target + correctness_is_independent_of_budget_both_ways \
         (an L5 SLO states window/population/statistic/error-budget/exclusions or fails closed, and \
         correctness gates sit outside the error budget — the promotion gate takes no budget input); \
         operator drill `entelechy drill --kind slo`",
        V,
    );

    // PV-2 and DP-1 are mechanized by the Phase-0 real-domain study
    // (entelechy-domain): designs are executed against a resettable helpdesk world
    // and scored by programmatic state checkers, so these are real verification
    // records, not waivers. No waivers remain in the current evidence set.
    index.verify(
        "PV-2",
        "entelechy-domain: study::real_study_is_green + release_evidence_is_the_post_drift_epoch_not_the_pool \
         + epoch::a_new_fingerprint_opens_a_new_epoch_never_pooling (every model call's ModelFingerprint keys an \
         EvidenceEpoch; a material revision drift opens a new epoch and the release reads only the current one, so \
         pre-drift evidence is never silently pooled); real-domain study `entelechy study --real`",
        V,
    );
    index.verify(
        "DP-1",
        "entelechy-domain: study::real_study_is_green + declaration_is_stale_if_the_profile_changes (a \
         DeploymentDeclaration names exactly one DeploymentProfile and carries a signed approval bound to it — \
         the single-deployment-profile approval step, revalidated like any approval); real-domain study \
         `entelechy study --real`",
        V,
    );

    // PV-3, EL-1, PC-2, DR-2, DR-3, OL-2 and DX-1 are first enforced at P2/P4. Each
    // is backed by real executed code with its own tests/drills, so the P4 gate is
    // covered with no waivers.
    index.verify(
        "PV-3",
        "entelechy-domain: promotion::drift_drill_satisfies_pv3 + regression_is_the_validation_drop \
         + within_tolerance_drift_is_allowed (a drift regressing behavior beyond a declared tolerance \
         blocks automatic promotion; a within-tolerance material drift and a recorded human override do \
         not); operator drill `entelechy drill --kind drift`",
        V,
    );
    index.verify(
        "EL-1",
        "entelechy-eval: holdout::{design_plane_is_refused, clear_improvement_passes_and_consumes_budget, \
         budget_exhaustion_refuses} (the holdout gate returns only coarse pass/fail, audits every query, \
         and enforces the query budget — a design/search plane is refused and budget exhaustion fails closed)",
        V,
    );
    index.verify(
        "PC-2",
        "entelechy-protocol: negotiate_secure + no_silent_security_downgrade (a negotiated session never \
         silently drops a required security guarantee; an unsatisfiable security requirement fails closed \
         with a machine-readable error)",
        V,
    );
    // PC-3 is the one requirement first enforced at P5; the P5 advance records it.
    // Its compatibility rule is verified in entelechy-protocol and enforced on the
    // real plugin loader in entelechy-gateway (so it is not a dead function).
    index.verify(
        "PC-3",
        "entelechy-protocol: plugin_manifest_declares_compatibility_and_loading_fails_closed (a manifest \
         declares host-API and artifact-schema windows plus required capabilities; an unsupported host \
         version or schema, a capability the host does not offer, or a manifest declaring no usable window \
         all fail closed — never a silent accept) + entelechy-gateway: \
         sandbox::incompatible_manifest_fails_closed_before_authority (PluginHost::load enforces the \
         manifest's declared compatibility against the host before any capability is granted — PC-3 ahead \
         of the no-ambient-authority model, PRD 16.6/16.3)",
        V,
    );
    index.verify(
        "DR-2",
        "entelechy-artifacts: backup::{revocation_survives_restore, post_restore_effects_are_reconciled_first} \
         (erasures and revocations survive a restore — never resurrected — and an in-flight effect whose commit \
         was unknown at backup time is reconciled before the restored state is ready for new work); operator \
         drill `entelechy drill --kind restore`",
        V,
    );
    index.verify(
        "DR-3",
        "entelechy-artifacts: platform::partial_loss_marks_evidence_incomplete + entelechy-release promote() \
         evidence_state gate (PromotionError::EvidenceIncomplete, not override-able) + cross-crate \
         entelechy-integration/tests/recovery.rs::partial_loss_marks_incomplete_and_blocks_promotion (a \
         partial-loss restore marks the evidence incomplete and blocks new promotion even with a recorded \
         override, until the evidence is regenerated)",
        V,
    );
    index.verify(
        "OL-2",
        "entelechy-ops: degraded::{degraded_mode_drill_satisfies_ol2, fallback_does_not_inherit_primary_authority, \
         degraded_observations_never_register} (a fallback is authorized on its own capabilities and \
         confidentiality tier — never inherited; entering degraded mode is journaled with a reason; degraded-mode \
         observations are excluded from registered evidence); operator drill `entelechy drill --kind degraded`",
        V,
    );
    index.verify(
        "DX-1",
        "entelechy-cli: quickstart::{quickstart_reaches_first_report_within_30_minutes, \
         the_last_step_produces_the_report} (the declared new-user path drives the Phase-0 real-domain study \
         to a green first study report with a total step budget under 30 minutes); `entelechy quickstart`",
        V,
    );

    index
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

    #[test]
    fn phase_gate_evidence_passes_the_current_gate() {
        // Before the waiver review deadline, the declared evidence (records +
        // active waivers) closes every gap at the current gate phase (TR-1).
        let index = phase_gate_evidence();
        let before_deadline = WAIVER_REVIEW_DEADLINE - 1;
        let gaps = index.phase_gate_gaps(CURRENT_GATE_PHASE, before_deadline);
        assert!(gaps.is_empty(), "unexpected phase-gate gaps: {gaps:?}");
    }

    #[test]
    fn every_required_requirement_is_recorded_or_waived() {
        // Time-independent coverage: a requirement first enforced at or before the
        // gate phase must ship with a verification record or a waiver, so adding a
        // new one to the registry without evidence fails here.
        let index = phase_gate_evidence();
        for r in REGISTRY
            .iter()
            .filter(|r| r.first_enforced <= CURRENT_GATE_PHASE)
        {
            let recorded = index.records.iter().any(|rec| rec.requirement_id == r.id);
            let waived = index.waivers.iter().any(|w| w.requirement_id == r.id);
            assert!(
                recorded || waived,
                "{} has neither a verification record nor a waiver",
                r.id
            );
        }
    }

    #[test]
    fn waived_requirements_are_all_permissibly_waivable() {
        // A waiver may never stand in for a StaticProof requirement (PRD 17.5).
        let index = phase_gate_evidence();
        for w in &index.waivers {
            let req = crate::find(&w.requirement_id).expect("waiver names a real requirement");
            assert!(
                is_waivable(req),
                "{} is non-waivable (StaticProof) and must not be waived",
                w.requirement_id
            );
        }
    }
}
