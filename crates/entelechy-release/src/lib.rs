//! Release bundles, maturity promotion, rollback and revocation.
//!
//! PRD v11 references: section 14 (assurance & release), 14.2 (maturity levels),
//! 14.3 (promotion rules), 14.4 (release bundle), 14.5 (approval integrity),
//! 14.6 (rollback & revocation), Appendix A (lifecycle), RB-1/RB-2, Q24.
//!
//! Promotion is monotonic through maturity levels unless an authorized override is
//! recorded (14.3). A holdout regression or negative-goal violation blocks
//! ordinary promotion. Approvals are revalidated immediately before promotion
//! (14.5). Rollback and revocation authority is separate from promotion authority
//! (14.6): an on-call operator can stop a release without holding release-approval
//! rights.
#![forbid(unsafe_code)]

pub mod maturity;

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use entelechy_assurance::{AssuranceReport, SafetyCase};
use entelechy_identity::{Approval, ApprovalBinding, ApprovalStatus, KeyRing, TimeSource};

pub use maturity::{LifecycleState, MaturityLevel};

/// A release bundle (PRD 14.4): everything needed to execute, replay and defend a
/// release. Fields are content-address references to immutable artifacts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReleaseBundle {
    /// Design IR content hash.
    pub design_id: String,
    /// GoalSpec content hash.
    pub goalspec_id: String,
    /// AuthorityEnvelope content hash.
    pub authority_id: String,
    /// EvalContract content hash.
    pub evalcontract_id: String,
    /// The SafetyCase.
    pub safety_case: SafetyCase,
    /// Policy snapshot version (PRD 5.2).
    pub policy_snapshot_version: u32,
    /// Model and tool identifiers (PRD 14.4, 5.10).
    pub model_tool_ids: Vec<String>,
    /// Provenance / lineage note.
    pub provenance: String,
    /// Rollback target (a previously approved release), or `None` with a recorded
    /// rationale (RB-2).
    pub rollback_target: Option<String>,
    /// Recorded no-rollback rationale when `rollback_target` is `None` (RB-2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_rollback_rationale: Option<String>,
}

impl ReleaseBundle {
    /// Schema id for the artifact (PRD 5.9).
    pub const SCHEMA_ID: &'static str = "entelechy.release";

    /// Content address of the bundle.
    pub fn id(&self) -> String {
        entelechy_artifacts::ArtifactId::of(self)
            .map(|i| i.to_string())
            .unwrap_or_default()
    }

    /// Whether this bundle satisfies the RB-2 rollback-readiness requirement.
    pub fn rollback_ready(&self) -> bool {
        self.rollback_target.is_some() || self.no_rollback_rationale.is_some()
    }
}

/// An authorized override of a blocked promotion (PRD 14.3). Must name the failed
/// criterion, rationale, approver and an expiry/review condition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromotionOverride {
    /// The criterion that failed.
    pub failed_criterion: String,
    /// Why the override is justified.
    pub rationale: String,
    /// The approving principal id.
    pub approver: String,
    /// Expiry / review timestamp (unix seconds).
    pub expires_at: u64,
}

/// The gate inputs a promotion is evaluated against (PRD 14.1, 14.3).
pub struct PromotionGate<'a> {
    /// The assurance report for the candidate.
    pub assurance: &'a AssuranceReport,
    /// Whether the holdout gate passed (no regression — EV-14).
    pub holdout_passed: bool,
    /// Whether negative-goal suites are clean (PRD 14.3).
    pub negative_goal_clean: bool,
}

/// Approval-revalidation inputs, checked immediately before promotion (PRD 14.5).
pub struct ApprovalCheck<'a> {
    /// The approvals presented.
    pub approvals: &'a [Approval],
    /// The current artifact binding to revalidate against (Q17).
    pub current_binding: &'a ApprovalBinding,
    /// The key ring (for signature + revocation).
    pub keyring: &'a KeyRing,
    /// The time source (fail closed on uncertainty — AU-3).
    pub clock: &'a dyn TimeSource,
}

impl ApprovalCheck<'_> {
    /// Number of currently-valid approvals (PRD 14.5 revalidation).
    fn valid_count(&self) -> usize {
        self.approvals
            .iter()
            .filter(|a| {
                a.status_for(self.current_binding, self.keyring, self.clock) == ApprovalStatus::Valid
            })
            .count()
    }

    /// Whether any presented approval is specifically stale (material change).
    fn any_stale(&self) -> bool {
        self.approvals.iter().any(|a| {
            matches!(
                a.status_for(self.current_binding, self.keyring, self.clock),
                ApprovalStatus::Stale { .. }
            )
        })
    }
}

/// Why a promotion was refused (PRD 14.3).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PromotionError {
    /// Promotion must be to the next level unless an override is recorded.
    #[error("non-monotonic promotion from {from:?} to {to:?} without override")]
    NonMonotonic {
        /// Current level.
        from: MaturityLevel,
        /// Requested level.
        to: MaturityLevel,
    },
    /// Static/SafetyCase assurance is not release-eligible.
    #[error("assurance not release-eligible")]
    AssuranceIncomplete,
    /// A holdout regression blocks ordinary promotion.
    #[error("holdout regression blocks promotion")]
    HoldoutRegression,
    /// A negative-goal violation blocks ordinary promotion.
    #[error("negative-goal violation blocks promotion")]
    NegativeGoalViolation,
    /// No currently-valid approval (or a stale one) at a safety-gated level.
    #[error("no valid approval for a safety-qualified promotion (14.5)")]
    MissingValidApproval,
    /// L4/L5 requires a rollback target or an approved no-rollback rationale.
    #[error("L4/L5 promotion requires a rollback target or approved rationale (RB-2)")]
    RollbackNotReady,
}

/// A recorded promotion decision (PRD 14.3, 14.4 approval chain).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Promotion {
    /// Release bundle id promoted.
    pub release_id: String,
    /// Level promoted from.
    pub from: MaturityLevel,
    /// Level promoted to.
    pub to: MaturityLevel,
    /// The override used, if any (PRD 14.3).
    pub override_used: Option<PromotionOverride>,
}

/// Evaluate a promotion from `from` to `to` (PRD 14.3, 14.5). Returns the
/// recorded decision or the reason it was refused.
pub fn promote(
    bundle: &ReleaseBundle,
    from: MaturityLevel,
    to: MaturityLevel,
    gate: &PromotionGate,
    approvals: &ApprovalCheck,
    override_: Option<PromotionOverride>,
) -> Result<Promotion, PromotionError> {
    let overridden = override_.is_some();

    // Monotonicity (PRD 14.3): to must be exactly the next level unless overridden.
    if from.next() != Some(to) && !overridden {
        return Err(PromotionError::NonMonotonic { from, to });
    }

    if to.requires_safety_gates() {
        // Assurance eligibility (PRD 14.1, 9.2).
        if !gate.assurance.release_eligible() && !overridden {
            return Err(PromotionError::AssuranceIncomplete);
        }
        // Holdout regression / negative goals block ordinary promotion (PRD 14.3).
        if !gate.holdout_passed && !overridden {
            return Err(PromotionError::HoldoutRegression);
        }
        if !gate.negative_goal_clean && !overridden {
            return Err(PromotionError::NegativeGoalViolation);
        }
        // Approvals revalidated at promotion (PRD 14.5). A stale approval is a
        // hard failure even under override — a material change needs re-approval.
        if approvals.any_stale() {
            return Err(PromotionError::MissingValidApproval);
        }
        if approvals.valid_count() == 0 {
            return Err(PromotionError::MissingValidApproval);
        }
    }

    // RB-2: production authority requires rollback readiness (not override-able).
    if to.is_production_authority() && !bundle.rollback_ready() {
        return Err(PromotionError::RollbackNotReady);
    }

    Ok(Promotion {
        release_id: bundle.id(),
        from,
        to,
        override_used: override_,
    })
}

/// Authority to act on deployments (PRD 14.6). Stop authority is separate from
/// promote authority.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeployAuthority {
    /// May promote / deploy.
    pub can_promote: bool,
    /// May stop: rollback or revoke.
    pub can_stop: bool,
}

/// An automatic rollback trigger declared in the release policy (PRD 14.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutoTrigger {
    /// A safety metric breached its bound.
    SafetyMetricBreach,
    /// A negative-goal violation was observed online.
    NegativeGoalViolation,
    /// Budget runaway.
    BudgetRunaway,
}

/// Errors from the deployment controller.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DeployError {
    /// The actor lacks promote authority.
    #[error("actor lacks promote authority")]
    NotPromoter,
    /// The actor lacks stop authority.
    #[error("actor lacks stop authority")]
    NotStopper,
    /// There is no previous release to roll back to.
    #[error("no rollback target available")]
    NoRollbackTarget,
}

/// Operational deployment state (PRD 14.6). Releases are immutable; rollback
/// creates a new deployment decision pointing to a previously approved release.
#[derive(Clone, Debug, Default)]
pub struct DeploymentController {
    active: Option<String>,
    history: Vec<String>,
    revoked: BTreeSet<String>,
    declared_triggers: Vec<AutoTrigger>,
}

impl DeploymentController {
    /// Create a controller with declared automatic rollback triggers (RB-1).
    pub fn new(declared_triggers: Vec<AutoTrigger>) -> Self {
        Self {
            declared_triggers,
            ..Default::default()
        }
    }

    /// The currently active release, if any and not revoked.
    pub fn active(&self) -> Option<&String> {
        self.active.as_ref().filter(|id| !self.revoked.contains(*id))
    }

    /// Deploy a release (requires promote authority — PRD 14.6).
    pub fn deploy(&mut self, release_id: impl Into<String>, actor: DeployAuthority) -> Result<(), DeployError> {
        if !actor.can_promote {
            return Err(DeployError::NotPromoter);
        }
        let id = release_id.into();
        if let Some(prev) = self.active.take() {
            self.history.push(prev);
        }
        self.active = Some(id);
        Ok(())
    }

    /// Roll back to the previous release (requires stop authority — PRD 14.6).
    /// Creates a new deployment decision; the failed release is not mutated.
    /// Re-promotion later requires approval (enforced by [`promote`]).
    pub fn rollback(&mut self, actor: DeployAuthority) -> Result<String, DeployError> {
        if !actor.can_stop {
            return Err(DeployError::NotStopper);
        }
        let target = self.history.pop().ok_or(DeployError::NoRollbackTarget)?;
        self.active = Some(target.clone());
        Ok(target)
    }

    /// Emergency revocation: make a release ineligible for new executions while
    /// preserving evidence (PRD 14.6). Requires stop authority.
    pub fn revoke(&mut self, release_id: impl Into<String>, actor: DeployAuthority) -> Result<(), DeployError> {
        if !actor.can_stop {
            return Err(DeployError::NotStopper);
        }
        self.revoked.insert(release_id.into());
        Ok(())
    }

    /// Whether a signal fires an automatic rollback (PRD 14.6): only declared
    /// triggers act without a human, because moving to a safer state is fail-safe.
    pub fn auto_rollback_fires(&self, trigger: AutoTrigger) -> bool {
        self.declared_triggers.contains(&trigger)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_assurance::AssuranceReport;
    use entelechy_identity::{Approval, Principal, PrincipalKind, FixedClock};
    use std::collections::BTreeMap;

    fn eligible_report() -> AssuranceReport {
        AssuranceReport {
            static_violations: vec![],
            safety_case: SafetyCase::default(),
            policy_snapshot_version: 1,
        }
    }

    fn bundle(rollback: bool) -> ReleaseBundle {
        ReleaseBundle {
            design_id: "sha256:d".into(),
            goalspec_id: "sha256:g".into(),
            authority_id: "sha256:a".into(),
            evalcontract_id: "sha256:e".into(),
            safety_case: SafetyCase::default(),
            policy_snapshot_version: 1,
            model_tool_ids: vec!["mock@r1".into()],
            provenance: "study X".into(),
            rollback_target: if rollback { Some("rel-prev".into()) } else { None },
            no_rollback_rationale: None,
        }
    }

    fn approval_setup() -> (KeyRing, ApprovalBinding, Vec<Approval>) {
        let mut ring = KeyRing::new();
        ring.add_key("k", "s");
        let binding: ApprovalBinding = BTreeMap::from([("design".to_string(), "sha256:d".to_string())]);
        let appr = Approval::sign(
            Principal::new("alice", PrincipalKind::Approver),
            binding.clone(),
            "k",
            &ring,
            10,
            Some(10_000),
        )
        .unwrap();
        (ring, binding, vec![appr])
    }

    #[test]
    fn l1_promotion_needs_no_safety_gate() {
        let b = bundle(false);
        let (ring, binding, appr) = approval_setup();
        let check = ApprovalCheck { approvals: &appr, current_binding: &binding, keyring: &ring, clock: &FixedClock(Some(20)) };
        let gate = PromotionGate { assurance: &eligible_report(), holdout_passed: true, negative_goal_clean: true };
        let p = promote(&b, MaturityLevel::L0, MaturityLevel::L1, &gate, &check, None).unwrap();
        assert_eq!(p.to, MaturityLevel::L1);
    }

    #[test]
    fn holdout_regression_blocks_l2() {
        let b = bundle(false);
        let (ring, binding, appr) = approval_setup();
        let check = ApprovalCheck { approvals: &appr, current_binding: &binding, keyring: &ring, clock: &FixedClock(Some(20)) };
        let gate = PromotionGate { assurance: &eligible_report(), holdout_passed: false, negative_goal_clean: true };
        let err = promote(&b, MaturityLevel::L1, MaturityLevel::L2, &gate, &check, None).unwrap_err();
        assert_eq!(err, PromotionError::HoldoutRegression);
    }

    #[test]
    fn stale_approval_blocks_even_with_override() {
        let b = bundle(true);
        let (ring, _binding, appr) = approval_setup();
        // Current binding differs -> approval is stale.
        let changed: ApprovalBinding = BTreeMap::from([("design".to_string(), "sha256:DIFF".to_string())]);
        let check = ApprovalCheck { approvals: &appr, current_binding: &changed, keyring: &ring, clock: &FixedClock(Some(20)) };
        let gate = PromotionGate { assurance: &eligible_report(), holdout_passed: true, negative_goal_clean: true };
        let ov = PromotionOverride { failed_criterion: "x".into(), rationale: "y".into(), approver: "z".into(), expires_at: 9_999 };
        let err = promote(&b, MaturityLevel::L1, MaturityLevel::L2, &gate, &check, Some(ov)).unwrap_err();
        assert_eq!(err, PromotionError::MissingValidApproval);
    }

    #[test]
    fn l5_requires_rollback_ready() {
        let b = bundle(false); // no rollback target, no rationale
        let (ring, binding, appr) = approval_setup();
        let check = ApprovalCheck { approvals: &appr, current_binding: &binding, keyring: &ring, clock: &FixedClock(Some(20)) };
        let gate = PromotionGate { assurance: &eligible_report(), holdout_passed: true, negative_goal_clean: true };
        let err = promote(&b, MaturityLevel::L4, MaturityLevel::L5, &gate, &check, None).unwrap_err();
        assert_eq!(err, PromotionError::RollbackNotReady);
    }

    #[test]
    fn stop_authority_is_separate_from_promote() {
        let mut dc = DeploymentController::new(vec![AutoTrigger::NegativeGoalViolation]);
        let oncall = DeployAuthority { can_promote: false, can_stop: true };
        let releaser = DeployAuthority { can_promote: true, can_stop: false };
        // On-call cannot promote.
        assert_eq!(dc.deploy("rel-1", oncall), Err(DeployError::NotPromoter));
        // Releaser deploys two releases.
        dc.deploy("rel-1", releaser).unwrap();
        dc.deploy("rel-2", releaser).unwrap();
        assert_eq!(dc.active(), Some(&"rel-2".to_string()));
        // On-call can roll back without promote rights.
        let target = dc.rollback(oncall).unwrap();
        assert_eq!(target, "rel-1");
        assert_eq!(dc.active(), Some(&"rel-1".to_string()));
        // Releaser cannot stop.
        assert_eq!(dc.revoke("rel-1", releaser), Err(DeployError::NotStopper));
        // Declared auto-trigger fires; undeclared does not.
        assert!(dc.auto_rollback_fires(AutoTrigger::NegativeGoalViolation));
        assert!(!dc.auto_rollback_fires(AutoTrigger::BudgetRunaway));
    }

    #[test]
    fn revoked_release_is_not_active() {
        let mut dc = DeploymentController::new(vec![]);
        let releaser = DeployAuthority { can_promote: true, can_stop: true };
        dc.deploy("rel-1", releaser).unwrap();
        dc.revoke("rel-1", releaser).unwrap();
        assert_eq!(dc.active(), None);
    }
}
