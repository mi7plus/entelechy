//! A bundled online-operations demo (PRD §15, §21.4): feed a stream of production
//! run observations and user feedback through an [`OnlineEvalLoop`] and surface the
//! typed event trail it emits — sampling (OP-2), drift detection (OP-3), incident
//! capture (OP-4), permanent regression coverage (EV-12), bounded re-studies that
//! wait at the release gate (OP-5, §14.3) and the quota hard stop (OP-7).
//!
//! The loop is deterministic, so the same scenario always yields the same event
//! stream — an operator can replay and audit exactly why each incident and
//! re-study was opened.

use entelechy_ops::online::{OnlineEvalLoop, OpsEvent, OpsPolicy, RunObservation};
use entelechy_ops::{DriftKind, Feedback, QuotaScope};

/// A small-window policy that makes the demo's drift and feedback triggers fire
/// within a short, readable scenario (production windows are far larger).
fn demo_policy() -> OpsPolicy {
    OpsPolicy {
        eval_sample_rate: 1.0,
        drift_k: 3.0,
        baseline_window: 5,
        recent_window: 3,
        min_feedback: 4,
        satisfaction_floor: 0.8,
        restudy_quota_scope: QuotaScope::System,
        restudy_budget: Some(8.0),
    }
}

/// Run the bundled scenario, returning the loop (for its quota accounting) and the
/// full event stream it emitted, in order.
pub fn run_ops_demo() -> (OnlineEvalLoop, Vec<OpsEvent>) {
    let mut loopy = OnlineEvalLoop::new(demo_policy());
    let mut events = Vec::new();

    // 1. A steady cost baseline: five runs around 10 cost units. No drift fires
    //    while the baseline window fills (OP-2 sampling only).
    for i in 0..5 {
        events.extend(loopy.observe(RunObservation::one(
            format!("base-{i}"),
            DriftKind::Cost,
            10.0 + (i % 2) as f64 * 0.5,
            "trace://baseline",
        )));
    }

    // 2. A cost regression: three runs at ~50 units. The third fills the recent
    //    window and trips drift -> incident -> regression (EV-12) -> a bounded
    //    re-study (OP-3/OP-4/OP-5). The signal then latches: a sustained regime
    //    opens one re-study, not one per subsequent run.
    for i in 0..3 {
        events.extend(loopy.observe(RunObservation::one(
            format!("spike-{i}"),
            DriftKind::Cost,
            50.0,
            format!("trace://spike-{i}"),
        )));
    }

    // 3. User feedback: one happy, then three unhappy. On the fourth item the
    //    satisfaction rate (0.25) falls below the 0.8 floor and opens a second,
    //    feedback-driven incident + re-study (OP-5). Early items below the
    //    minimum count do not trip it.
    events.extend(loopy.ingest_feedback(Feedback {
        run_id: "spike-0".into(),
        positive: true,
        note: None,
    }));
    for i in 0..3 {
        events.extend(loopy.ingest_feedback(Feedback {
            run_id: format!("unhappy-{i}"),
            positive: false,
            note: Some("wrong answer".into()),
        }));
    }

    (loopy, events)
}

/// A one-line, human-readable description of an event for the CLI log, each
/// citing the PRD requirement it realizes.
pub fn describe(event: &OpsEvent) -> String {
    match event {
        OpsEvent::Sampled { run_id } => format!("sampled {run_id} for online eval (OP-2)"),
        OpsEvent::NotSampled { run_id } => format!("skipped {run_id} by sampling rate"),
        OpsEvent::DriftDetected { evidence } => format!(
            "DRIFT {:?}: baseline {:.2} -> window {:.2} ({:+.2} sigma) (OP-3)",
            evidence.kind, evidence.baseline_mean, evidence.window_mean, evidence.z_shift
        ),
        OpsEvent::SatisfactionBreach { satisfaction } => {
            format!("user satisfaction {satisfaction:.2} fell below the floor (OP-5)")
        }
        OpsEvent::IncidentOpened { incident } => {
            format!("incident {} opened: {} (OP-4)", incident.id, incident.summary)
        }
        OpsEvent::RegressionCaptured { task } => format!(
            "regression {} captured from {} — permanent coverage (EV-12)",
            task.id, task.source_incident
        ),
        OpsEvent::ReStudyOpened { restudy } => format!(
            "re-study {} opened ({:?}); waits at the release gate, never auto-promotes (OP-5, §14.3)",
            restudy.id, restudy.reason
        ),
        OpsEvent::ReStudyQuotaBlocked { reason } => {
            format!("re-study BLOCKED by the quota hard stop: {reason} (OP-7)")
        }
    }
}
