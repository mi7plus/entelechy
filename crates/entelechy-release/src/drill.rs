//! The rollback drill (PRD §14.6, RB-1/RB-2): a single composed operational drill
//! that proves the deployment controller recovers a production rollout to a
//! known-good release, that stop authority is separate from promote authority, and
//! that a production release without a tested rollback target (or an approved
//! no-rollback rationale) is refused.
//!
//! The crate's unit tests already exercise each primitive in isolation; this drill
//! is the *operational drill* those RB requirements call for (their registry
//! verification kind) — it runs the real promote → deploy → trigger → rollback →
//! revoke sequence end to end and reports the recovered state, so an operator can
//! run it (and persist the report) as a disaster-recovery exercise.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use entelechy_assurance::{AssuranceReport, SafetyCase};
use entelechy_identity::{
    Approval, ApprovalBinding, FixedClock, KeyRing, Principal, PrincipalKind,
};

use crate::{
    promote, ApprovalCheck, AutoTrigger, DeployAuthority, DeployError, DeploymentController,
    MaturityLevel, PromotionError, PromotionGate, ReleaseBundle,
};

/// The outcome of the rollback drill (PRD §14.6, RB-1/RB-2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackDrillReport {
    /// RB-2: a production (L4/L5) promotion with no rollback target and no
    /// rationale is refused, while an otherwise-identical rollback-ready bundle is
    /// allowed through the same gate.
    pub rollback_ready_enforced: bool,
    /// RB-2: an approved no-rollback *rationale* also satisfies readiness (the
    /// documented-exception path), not only a concrete rollback target.
    pub rationale_satisfies_rb2: bool,
    /// RB-1: stop authority is separate from promote authority — an on-call stopper
    /// cannot deploy, and a releaser cannot roll back or revoke.
    pub authority_separation_enforced: bool,
    /// RB-1: the automatic rollback triggers declared in the release policy.
    pub declared_triggers: Vec<AutoTrigger>,
    /// RB-1: a declared trigger fires an automatic rollback.
    pub declared_trigger_fired: bool,
    /// RB-1: an undeclared signal does *not* fire an automatic rollback.
    pub undeclared_trigger_ignored: bool,
    /// The known-good release the rollback recovered to.
    pub recovered_to: String,
    /// Whether the rollback restored the prior known-good release as active.
    pub recovery_ok: bool,
    /// Whether emergency revocation of the active release makes the system serve
    /// nothing (fail-safe stop — the active release becomes ineligible).
    pub revocation_ok: bool,
    /// Whether every RB-1/RB-2 invariant held.
    pub ok: bool,
}

/// Run the rollback drill (PRD §14.6). Deterministic: a fixed clock, keyring and
/// release ids mean the same inputs always yield the same report.
pub fn rollback_drill() -> RollbackDrillReport {
    // --- RB-2: production promotion requires a tested rollback target or an
    //     approved no-rollback rationale. Run the *real* promotion gate at L4→L5
    //     (both production authority) so the readiness check is reached only after
    //     the safety and approval gates pass.
    let report = eligible_report();
    let (keyring, binding, approvals) = valid_approval();
    let clock = FixedClock(Some(1_000_060));
    let gate = PromotionGate {
        assurance: &report,
        holdout_passed: true,
        negative_goal_clean: true,
    };
    let check = ApprovalCheck {
        approvals: &approvals,
        current_binding: &binding,
        keyring: &keyring,
        clock: &clock,
    };
    let promote_l5 = |bundle: &ReleaseBundle| {
        promote(
            bundle,
            MaturityLevel::L4,
            MaturityLevel::L5,
            &gate,
            &check,
            None,
        )
    };
    let unready_refused = matches!(
        promote_l5(&bundle(None, None)),
        Err(PromotionError::RollbackNotReady)
    );
    let target_allowed = promote_l5(&bundle(Some("rel-baseline"), None)).is_ok();
    let rationale_allowed =
        promote_l5(&bundle(None, Some("single-shot, no safe predecessor"))).is_ok();
    let rollback_ready_enforced = unready_refused && target_allowed;
    let rationale_satisfies_rb2 = rationale_allowed;

    // --- RB-1: stop authority is separate from promote authority.
    let oncall = DeployAuthority {
        can_promote: false,
        can_stop: true,
    };
    let releaser = DeployAuthority {
        can_promote: true,
        can_stop: false,
    };
    let mut probe = DeploymentController::new(vec![]);
    let oncall_cannot_deploy =
        matches!(probe.deploy("rel-x", oncall), Err(DeployError::NotPromoter));
    probe
        .deploy("rel-x", releaser)
        .expect("releaser may deploy");
    let releaser_cannot_rollback = matches!(probe.rollback(releaser), Err(DeployError::NotStopper));
    let releaser_cannot_revoke = matches!(
        probe.revoke("rel-x", releaser),
        Err(DeployError::NotStopper)
    );
    let authority_separation_enforced =
        oncall_cannot_deploy && releaser_cannot_rollback && releaser_cannot_revoke;

    // --- RB-1: declared automatic rollback triggers, and the rollback recovery.
    let declared_triggers = vec![
        AutoTrigger::SafetyMetricBreach,
        AutoTrigger::NegativeGoalViolation,
    ];
    let mut dc = DeploymentController::new(declared_triggers.clone());
    dc.deploy("rel-good", releaser)
        .expect("releaser deploys the known-good baseline");
    dc.deploy("rel-bad", releaser)
        .expect("releaser deploys the regressing candidate");
    let declared_trigger_fired = dc.auto_rollback_fires(AutoTrigger::NegativeGoalViolation);
    let undeclared_trigger_ignored = !dc.auto_rollback_fires(AutoTrigger::BudgetRunaway);
    // On-call (stop authority only) rolls the regression back to the known-good.
    let recovered_to = dc.rollback(oncall).unwrap_or_default();
    let recovery_ok = recovered_to == "rel-good" && dc.active() == Some(&"rel-good".to_string());

    // --- RB-1: emergency revocation is a fail-safe stop — the active release
    //     becomes ineligible and the system serves nothing rather than something
    //     unsafe. Use a fresh controller so this is an independent demonstration.
    let mut emergency = DeploymentController::new(vec![]);
    emergency
        .deploy("rel-live", releaser)
        .expect("releaser deploys");
    emergency
        .revoke("rel-live", oncall)
        .expect("on-call may revoke");
    let revocation_ok = emergency.active().is_none();

    let ok = rollback_ready_enforced
        && rationale_satisfies_rb2
        && authority_separation_enforced
        && declared_trigger_fired
        && undeclared_trigger_ignored
        && recovery_ok
        && revocation_ok;

    RollbackDrillReport {
        rollback_ready_enforced,
        rationale_satisfies_rb2,
        authority_separation_enforced,
        declared_triggers,
        declared_trigger_fired,
        undeclared_trigger_ignored,
        recovered_to,
        recovery_ok,
        revocation_ok,
        ok,
    }
}

/// A release-eligible assurance report (no static violations, default safety case).
fn eligible_report() -> AssuranceReport {
    AssuranceReport {
        static_violations: vec![],
        safety_case: SafetyCase::default(),
        policy_snapshot_version: 1,
    }
}

/// Build a release bundle that differs only in its rollback readiness.
fn bundle(rollback_target: Option<&str>, no_rollback_rationale: Option<&str>) -> ReleaseBundle {
    ReleaseBundle {
        design_id: "sha256:d".into(),
        goalspec_id: "sha256:g".into(),
        authority_id: "sha256:a".into(),
        evalcontract_id: "sha256:e".into(),
        safety_case: SafetyCase::default(),
        policy_snapshot_version: 1,
        model_tool_ids: vec!["mock@r1".into()],
        provenance: "rollback-drill".into(),
        rollback_target: rollback_target.map(Into::into),
        no_rollback_rationale: no_rollback_rationale.map(Into::into),
    }
}

/// A fixed, currently-valid hash-bound approval over the bundle's design (PRD 14.5).
fn valid_approval() -> (KeyRing, ApprovalBinding, Vec<Approval>) {
    let mut ring = KeyRing::new();
    ring.add_key("k", "s");
    let binding: ApprovalBinding = BTreeMap::from([("design".to_string(), "sha256:d".to_string())]);
    let approval = Approval::sign(
        Principal::new("approver", PrincipalKind::Approver),
        binding.clone(),
        "k",
        &ring,
        1_000_000,
        Some(2_000_000),
    )
    .expect("signing key present");
    (ring, binding, vec![approval])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_drill_satisfies_rb1_and_rb2() {
        let r = rollback_drill();
        assert!(r.ok, "every RB-1/RB-2 invariant must hold: {r:?}");
        // RB-2.
        assert!(r.rollback_ready_enforced);
        assert!(r.rationale_satisfies_rb2);
        // RB-1 authority separation.
        assert!(r.authority_separation_enforced);
        // RB-1 declared triggers.
        assert_eq!(r.declared_triggers.len(), 2);
        assert!(r.declared_trigger_fired);
        assert!(r.undeclared_trigger_ignored);
        // Recovery + revocation.
        assert_eq!(r.recovered_to, "rel-good");
        assert!(r.recovery_ok);
        assert!(r.revocation_ok);
    }

    #[test]
    fn the_drill_is_deterministic() {
        assert_eq!(rollback_drill(), rollback_drill());
    }
}
