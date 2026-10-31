//! Drift-tolerance gating of automatic promotion (PRD 5.10, §21; PV-3).
//!
//! PV-2 (see [`crate::epoch`]) keeps evidence from being *silently pooled* across a
//! material model drift. PV-3 is the release-time consequence: when the drift
//! between the evidence a release was gathered under and the model now in force
//! regresses behavior beyond a declared tolerance, the *automatic* (unattended)
//! promotion path is blocked. A human with promote authority can still promote
//! behind an explicit recorded override — PV-3 blocks automatic promotion, not
//! every promotion.
//!
//! Drift magnitude is read from the real executed evidence: the success rate of the
//! pre-drift evidence epoch versus the post-drift (current) one. A material
//! fingerprint change that does not regress behavior beyond tolerance (for example,
//! a drift that only touches a failure mode the design does not rely on) still
//! permits automatic promotion; one that regresses beyond tolerance does not.

use entelechy_gateway::{ModelGateway, ModelRequest, ProviderIdentity};
use serde::{Deserialize, Serialize};

use crate::epoch::{EvidenceLedger, ModelFingerprint, RevisionModel};
use crate::exec::resolves;
use crate::study::{helpdesk_suite, repaired_design};
use entelechy_eval::{Split, Task};

/// The maximum behavioral regression (in percentage points of task success) a
/// material drift may introduce before automatic promotion is blocked (PV-3).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DriftTolerance {
    /// Maximum allowed success-rate drop, in percentage points, from the pre-drift
    /// evidence to the current evidence before automatic promotion is blocked.
    pub max_regression_pp: f64,
}

impl DriftTolerance {
    /// A tolerance with the given maximum regression in percentage points.
    pub const fn new(max_regression_pp: f64) -> Self {
        Self { max_regression_pp }
    }
}

impl Default for DriftTolerance {
    /// A conservative default: more than 10pp of regression blocks automatic
    /// promotion.
    fn default() -> Self {
        Self::new(10.0)
    }
}

/// What the evidence ledger says about drift since the pre-drift epoch (PV-3).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DriftAssessment {
    /// Whether a material model drift opened a new evidence epoch.
    pub drift_detected: bool,
    /// Behavioral regression from the pre-drift epoch to the current one, in
    /// percentage points (0 when no drift, or when behavior did not regress).
    pub regression_pp: f64,
}

/// Assess drift from an evidence ledger (PV-3): compares the current (post-drift)
/// epoch's success rate to the epoch immediately before it. A single epoch means no
/// drift and no regression.
pub fn assess_drift(ledger: &EvidenceLedger) -> DriftAssessment {
    let epochs = ledger.epochs();
    if epochs.len() < 2 {
        return DriftAssessment {
            drift_detected: ledger.drift_detected(),
            regression_pp: 0.0,
        };
    }
    let prev = &epochs[epochs.len() - 2];
    let cur = &epochs[epochs.len() - 1];
    let regression_pp = ((prev.success_rate() - cur.success_rate()) * 100.0).max(0.0);
    DriftAssessment {
        drift_detected: true,
        regression_pp,
    }
}

/// The decision of the automatic promotion path under a drift tolerance (PV-3).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutoPromotionDecision {
    /// Automatic promotion is permitted (no drift, or drift within tolerance).
    Allowed,
    /// Drift regressed behavior beyond tolerance; automatic promotion is blocked.
    BlockedByDrift {
        /// The observed regression in percentage points.
        regression_pp: f64,
        /// The tolerance it exceeded.
        tolerance_pp: f64,
    },
}

impl AutoPromotionDecision {
    /// Whether automatic promotion is permitted.
    pub fn is_allowed(&self) -> bool {
        matches!(self, AutoPromotionDecision::Allowed)
    }
}

/// The automatic promotion gate (PV-3): blocks automatic promotion when a material
/// drift regressed behavior beyond `tolerance`.
pub fn auto_promote(ledger: &EvidenceLedger, tolerance: DriftTolerance) -> AutoPromotionDecision {
    let a = assess_drift(ledger);
    if a.drift_detected && a.regression_pp > tolerance.max_regression_pp {
        AutoPromotionDecision::BlockedByDrift {
            regression_pp: a.regression_pp,
            tolerance_pp: tolerance.max_regression_pp,
        }
    } else {
        AutoPromotionDecision::Allowed
    }
}

/// Whether a promotion proceeds once the human path is considered (PV-3): an
/// `Allowed` automatic decision always proceeds; a drift-blocked one proceeds only
/// behind an explicit, recorded human override. PV-3 blocks *automatic* promotion,
/// not a deliberate human one.
pub fn promotion_proceeds(decision: &AutoPromotionDecision, human_override_recorded: bool) -> bool {
    match decision {
        AutoPromotionDecision::Allowed => true,
        AutoPromotionDecision::BlockedByDrift { .. } => human_override_recorded,
    }
}

/// The result of the PV-3 drift drill — one boolean per invariant, like the other
/// operational drills.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DriftDrillReport {
    /// Whether a material drift was observed in the regressing evidence.
    pub drift_detected: bool,
    /// The behavioral regression the drift introduced (percentage points).
    pub regression_pp: f64,
    /// The tolerance in force (percentage points).
    pub tolerance_pp: f64,
    /// Drift beyond tolerance blocks the automatic promotion path.
    pub beyond_tolerance_blocks_auto: bool,
    /// A material drift that stays within tolerance still permits automatic promotion.
    pub within_tolerance_allows_auto: bool,
    /// No drift permits automatic promotion.
    pub no_drift_allows_auto: bool,
    /// A recorded human override still promotes a drift-blocked release; without one
    /// the block stands (PV-3 blocks automatic promotion only).
    pub human_override_still_promotes: bool,
    /// Whether every PV-3 invariant held.
    pub ok: bool,
}

/// Run the PV-3 drift drill (PRD 5.10, §21): build real executed evidence under a
/// calibrated model revision and a silently-drifted one, and prove that a drift
/// beyond tolerance blocks automatic promotion while a within-tolerance drift and a
/// human override do not.
pub fn drift_drill() -> DriftDrillReport {
    let tolerance = DriftTolerance::default();
    let suite = helpdesk_suite();
    let calibrated = RevisionModel::new("mock-r1");
    let drifted = RevisionModel::new("mock-r2");
    let fp_calibrated = fingerprint(&calibrated);
    let fp_drifted = fingerprint(&drifted);

    // Regressing evidence: the full validation split (reply + decomposition) under
    // the calibrated revision then the drifted one. The drift breaks grounding, so
    // the reply tickets regress — a material, behavior-regressing drift.
    let val: Vec<&Task> = suite.split(Split::Validation).collect();
    let mut regressing = EvidenceLedger::new();
    regressing.observe(&fp_calibrated, &outcomes(&val, &calibrated));
    regressing.observe(&fp_drifted, &outcomes(&val, &drifted));
    let assessment = assess_drift(&regressing);
    let regressing_decision = auto_promote(&regressing, tolerance);
    let beyond_tolerance_blocks_auto = !regressing_decision.is_allowed();

    // Within-tolerance evidence: decomposition-only tasks, which the committee
    // resolves regardless of grounding — so the same material drift does not
    // regress behavior and automatic promotion is still permitted.
    let decomposition: Vec<&Task> = suite
        .split(Split::Validation)
        .filter(|t| t.has_tag("needs_decomposition"))
        .collect();
    let mut steady = EvidenceLedger::new();
    steady.observe(&fp_calibrated, &outcomes(&decomposition, &calibrated));
    steady.observe(&fp_drifted, &outcomes(&decomposition, &drifted));
    let within_tolerance_allows_auto =
        steady.drift_detected() && auto_promote(&steady, tolerance).is_allowed();

    // No-drift evidence: a single epoch under the calibrated revision.
    let mut no_drift = EvidenceLedger::new();
    no_drift.observe(&fp_calibrated, &outcomes(&val, &calibrated));
    let no_drift_allows_auto =
        !no_drift.drift_detected() && auto_promote(&no_drift, tolerance).is_allowed();

    // Human override: the drift-blocked decision proceeds only with a recorded
    // override, and the block stands without one.
    let human_override_still_promotes = promotion_proceeds(&regressing_decision, true)
        && !promotion_proceeds(&regressing_decision, false);

    let ok = assessment.drift_detected
        && beyond_tolerance_blocks_auto
        && within_tolerance_allows_auto
        && no_drift_allows_auto
        && human_override_still_promotes;

    DriftDrillReport {
        drift_detected: assessment.drift_detected,
        regression_pp: assessment.regression_pp,
        tolerance_pp: tolerance.max_regression_pp,
        beyond_tolerance_blocks_auto,
        within_tolerance_allows_auto,
        no_drift_allows_auto,
        human_override_still_promotes,
        ok,
    }
}

/// Resolve the repaired design over `tasks` under `gateway`, collecting outcomes.
fn outcomes(tasks: &[&Task], gateway: &dyn ModelGateway) -> Vec<bool> {
    let design = repaired_design();
    tasks
        .iter()
        .map(|t| resolves(&design, t, gateway))
        .collect()
}

/// The fingerprint a gateway produces from an identity-probe call.
fn fingerprint(gateway: &dyn ModelGateway) -> ModelFingerprint {
    let req = ModelRequest {
        model: "mock-small".into(),
        prompt: "identity-probe".into(),
        temperature: 0.0,
    };
    match gateway.infer(&req) {
        Ok(resp) => ModelFingerprint::of(&resp.identity),
        Err(_) => ModelFingerprint::of(&ProviderIdentity {
            provider: "unknown".into(),
            advertised_model: String::new(),
            endpoint: String::new(),
            request_schema_version: String::new(),
            sampling: serde_json::Value::Null,
            revision: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drift_drill_satisfies_pv3() {
        let r = drift_drill();
        assert!(r.drift_detected, "a material drift was observed");
        assert!(
            r.regression_pp > r.tolerance_pp,
            "the drift regressed beyond tolerance: {} <= {}",
            r.regression_pp,
            r.tolerance_pp
        );
        assert!(
            r.beyond_tolerance_blocks_auto,
            "drift beyond tolerance blocks automatic promotion"
        );
        assert!(
            r.within_tolerance_allows_auto,
            "a within-tolerance drift still permits automatic promotion"
        );
        assert!(
            r.no_drift_allows_auto,
            "no drift permits automatic promotion"
        );
        assert!(
            r.human_override_still_promotes,
            "a human override still promotes a drift-blocked release"
        );
        assert!(r.ok, "every PV-3 invariant held");
    }

    #[test]
    fn drift_drill_is_deterministic() {
        assert_eq!(drift_drill(), drift_drill());
    }

    #[test]
    fn regression_is_the_validation_drop() {
        // Validation split: 3 reply + 3 decomposition. Calibrated 6/6; drifted 3/6
        // (reply tickets lose grounding). Regression = 50pp.
        let r = drift_drill();
        assert!((r.regression_pp - 50.0).abs() < 1e-9, "{}", r.regression_pp);
    }

    #[test]
    fn within_tolerance_drift_is_allowed() {
        // A drift that does not regress behavior (0pp) is within any non-negative
        // tolerance, so automatic promotion proceeds even though a drift occurred.
        let calibrated = RevisionModel::new("mock-r1");
        let drifted = RevisionModel::new("mock-r2");
        let suite = helpdesk_suite();
        let dec: Vec<&Task> = suite
            .split(Split::Validation)
            .filter(|t| t.has_tag("needs_decomposition"))
            .collect();
        let mut ledger = EvidenceLedger::new();
        ledger.observe(&fingerprint(&calibrated), &outcomes(&dec, &calibrated));
        ledger.observe(&fingerprint(&drifted), &outcomes(&dec, &drifted));
        assert!(ledger.drift_detected());
        assert_eq!(assess_drift(&ledger).regression_pp, 0.0);
        assert!(auto_promote(&ledger, DriftTolerance::default()).is_allowed());
    }
}
