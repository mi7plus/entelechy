//! Release maturity levels and the lifecycle state machine (PRD 14.2, App. A).

use serde::{Deserialize, Serialize};

/// Release maturity levels (PRD 14.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MaturityLevel {
    /// L0 Experimental: compiler-valid; dev evaluation only.
    L0,
    /// L1 Evaluation-qualified: EvalContract approved; tune results + uncertainty.
    L1,
    /// L2 Safety-qualified: hard constraints, negative goals, policy and
    /// adversarial gates pass.
    L2,
    /// L3 Shadow: production inputs observed without consequential effects.
    L3,
    /// L4 Canary: bounded production authority and monitored rollout.
    L4,
    /// L5 Production: release approval, tested rollback, online eval, incidents.
    L5,
}

impl MaturityLevel {
    /// The next level up, if any.
    pub fn next(self) -> Option<MaturityLevel> {
        use MaturityLevel::*;
        Some(match self {
            L0 => L1,
            L1 => L2,
            L2 => L3,
            L3 => L4,
            L4 => L5,
            L5 => return None,
        })
    }

    /// The previous level down, if any (reverse transitions — App. A).
    pub fn prev(self) -> Option<MaturityLevel> {
        use MaturityLevel::*;
        Some(match self {
            L0 => return None,
            L1 => L0,
            L2 => L1,
            L3 => L2,
            L4 => L3,
            L5 => L4,
        })
    }

    /// A one-line description of the required evidence (PRD 14.2).
    pub fn required_evidence(self) -> &'static str {
        use MaturityLevel::*;
        match self {
            L0 => "compiler-valid; dev evaluation only",
            L1 => "EvalContract approved; tune results and uncertainty reported",
            L2 => "hard constraints, negative goals, policy and adversarial gates pass",
            L3 => "production inputs observed without consequential effects",
            L4 => "bounded production authority and monitored rollout",
            L5 => "release approval, tested rollback, online evaluation, incident capture",
        }
    }

    /// Whether this level exercises real production authority (L4/L5), which
    /// requires a rollback target or an approved no-rollback rationale (RB-2).
    pub fn is_production_authority(self) -> bool {
        matches!(self, MaturityLevel::L4 | MaturityLevel::L5)
    }

    /// Whether reaching this level requires the safety gates (L2 and above).
    pub fn requires_safety_gates(self) -> bool {
        self >= MaturityLevel::L2
    }
}

/// The core lifecycle state (PRD Appendix A).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LifecycleState {
    /// Drafting the objective.
    DraftObjective,
    /// Signed GoalSpec.
    SignedGoalSpec,
    /// Approved EvalContract.
    ApprovedEvalContract,
    /// Ready to run a study.
    StudyReady,
    /// A study is running.
    StudyRunning,
    /// A candidate exists.
    Candidate,
    /// An assured candidate (holdout evaluated, no unresolved hard-constraint fail).
    AssuredCandidate,
    /// Promoted to a maturity level.
    Released(MaturityLevel),
}

impl LifecycleState {
    /// Whether a forward or defined reverse transition to `to` is allowed
    /// (PRD Appendix A, including reverse transitions).
    pub fn can_transition(self, to: LifecycleState) -> bool {
        use LifecycleState::*;
        match (self, to) {
            (DraftObjective, SignedGoalSpec) => true,
            (SignedGoalSpec, ApprovedEvalContract) => true,
            (ApprovedEvalContract, StudyReady) => true,
            (StudyReady, StudyRunning) => true,
            (StudyRunning, Candidate) => true,
            (Candidate, AssuredCandidate) => true,
            (AssuredCandidate, Released(MaturityLevel::L0)) => true,
            (Released(a), Released(b)) => {
                // Forward one level, or reverse (rollback / invalidation).
                a.next() == Some(b) || b.next() == Some(a)
            }
            // Approval invalidation returns any released state to AssuredCandidate.
            (Released(_), AssuredCandidate) => true,
            // Material change branches lineage back to a new draft.
            (_, DraftObjective) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_ordered() {
        assert!(MaturityLevel::L5 > MaturityLevel::L2);
        assert_eq!(MaturityLevel::L2.next(), Some(MaturityLevel::L3));
        assert_eq!(MaturityLevel::L5.next(), None);
        assert_eq!(MaturityLevel::L0.prev(), None);
    }

    #[test]
    fn production_and_safety_flags() {
        assert!(MaturityLevel::L4.is_production_authority());
        assert!(!MaturityLevel::L3.is_production_authority());
        assert!(MaturityLevel::L2.requires_safety_gates());
        assert!(!MaturityLevel::L1.requires_safety_gates());
    }

    #[test]
    fn lifecycle_transitions_and_reverses() {
        use LifecycleState::*;
        assert!(AssuredCandidate.can_transition(Released(MaturityLevel::L0)));
        assert!(Released(MaturityLevel::L3).can_transition(Released(MaturityLevel::L4)));
        // Reverse (rollback).
        assert!(Released(MaturityLevel::L5).can_transition(Released(MaturityLevel::L4)));
        // Approval invalidation.
        assert!(Released(MaturityLevel::L4).can_transition(AssuredCandidate));
        // Illegal jump.
        assert!(!Released(MaturityLevel::L0).can_transition(Released(MaturityLevel::L3)));
    }
}
