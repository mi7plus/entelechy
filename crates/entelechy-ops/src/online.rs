//! The online operations loop (PRD §15: OP-1..OP-7, EV-12, §21.4).
//!
//! The primitives in the crate root are stateless: a sampling decision, a drift
//! statistic, a feedback aggregate, a quota ledger. This module is the *running
//! loop* that composes them — the P4 "measure and operate in production" story.
//!
//! [`OnlineEvalLoop`] consumes a stream of production [`RunObservation`]s and
//! user [`Feedback`], and turns them into a typed, auditable [`OpsEvent`] stream:
//!
//! - Each run is sampled for online evaluation at the configured rate (OP-2);
//!   only sampled runs feed the drift windows, so evaluation cost is bounded.
//! - Per drift signal it keeps a frozen baseline window and a rolling recent
//!   window, and re-runs [`crate::detect_drift`] as new values arrive (OP-3).
//! - On detected drift — or a user-satisfaction breach — it captures an
//!   [`Incident`] from the triggering trace, turns it into permanent regression
//!   coverage (OP-4, EV-12), and opens a *bounded* [`ReStudy`] whose candidates
//!   wait at the release gate and never auto-promote (OP-5, §14.3).
//! - Every re-study is charged against a [`crate::QuotaLedger`]; once the budget
//!   is spent the loop emits a hard-stop event instead of opening another one
//!   (OP-7, §17.4).
//!
//! The loop is deterministic: the same observations and feedback in the same
//! order always produce the same event stream, so an operator can replay and
//! audit exactly why each incident and re-study was opened.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    detect_drift, sample_for_eval, DriftEvidence, DriftKind, Feedback, FeedbackStore, Incident,
    QuotaLedger, QuotaScope, ReStudy, ReStudyReason, RegressionTask,
};

/// Policy for the online operations loop (PRD §15).
#[derive(Clone, Debug, PartialEq)]
pub struct OpsPolicy {
    /// Fraction of runs evaluated online (OP-2). Lower it to bound eval cost;
    /// the decision is deterministic per run id.
    pub eval_sample_rate: f64,
    /// Drift threshold in baseline standard deviations (OP-3).
    pub drift_k: f64,
    /// Number of initial sampled values that form a signal's frozen baseline.
    pub baseline_window: usize,
    /// Number of most-recent sampled values compared against the baseline.
    pub recent_window: usize,
    /// Minimum feedback count before the satisfaction floor is evaluated (so a
    /// single early complaint cannot trip a re-study).
    pub min_feedback: usize,
    /// Satisfaction rate below which a feedback-driven re-study opens (OP-5).
    pub satisfaction_floor: f64,
    /// The scope re-studies are charged against (OP-7).
    pub restudy_quota_scope: QuotaScope,
    /// Maximum re-studies before the hard stop; `None` is unbounded (OP-7).
    pub restudy_budget: Option<f64>,
}

impl Default for OpsPolicy {
    fn default() -> Self {
        Self {
            eval_sample_rate: 1.0,
            drift_k: 3.0,
            baseline_window: 20,
            recent_window: 10,
            min_feedback: 10,
            satisfaction_floor: 0.8,
            restudy_quota_scope: QuotaScope::System,
            // A hard cap so a storm of drift signals cannot open unbounded
            // re-studies (OP-7). Operators raise it deliberately.
            restudy_budget: Some(8.0),
        }
    }
}

/// One observed production run and its per-signal metrics (OP-2/OP-3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunObservation {
    /// The run id (drives the deterministic sampling decision).
    pub run_id: String,
    /// Metric value per drift signal for this run (e.g. cost, a quality score).
    pub metrics: BTreeMap<DriftKind, f64>,
    /// Reference to this run's trace bundle, captured if it triggers an incident.
    #[serde(default)]
    pub trace_ref: String,
}

impl RunObservation {
    /// A run observation with one signal value and a trace reference.
    pub fn one(
        run_id: impl Into<String>,
        kind: DriftKind,
        value: f64,
        trace_ref: impl Into<String>,
    ) -> Self {
        let mut metrics = BTreeMap::new();
        metrics.insert(kind, value);
        Self {
            run_id: run_id.into(),
            metrics,
            trace_ref: trace_ref.into(),
        }
    }
}

/// A typed event emitted by the loop — the audit trail of every decision
/// (PRD §15). Internally tagged so a stream serializes to a readable log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "event")]
pub enum OpsEvent {
    /// A run was selected for online evaluation (OP-2).
    Sampled {
        /// The sampled run.
        run_id: String,
    },
    /// A run was skipped by the sampling rate (recorded so the realized rate is
    /// auditable).
    NotSampled {
        /// The skipped run.
        run_id: String,
    },
    /// Drift was detected with evidence (OP-3).
    DriftDetected {
        /// The drift statistic and means.
        evidence: DriftEvidence,
    },
    /// User satisfaction fell below the policy floor (OP-5 feedback trigger).
    SatisfactionBreach {
        /// The breaching satisfaction rate.
        satisfaction: f64,
    },
    /// An incident was captured from a trace bundle (OP-4).
    IncidentOpened {
        /// The captured incident.
        incident: Incident,
    },
    /// The incident became permanent regression coverage (OP-4, EV-12).
    RegressionCaptured {
        /// The derived regression task.
        task: RegressionTask,
    },
    /// A bounded re-study was opened; its candidates wait at the release gate and
    /// never auto-promote (OP-5, §14.3).
    ReStudyOpened {
        /// The opened re-study.
        restudy: ReStudy,
    },
    /// A re-study could not be opened because the quota is exhausted — a hard
    /// stop, not a model decision (OP-7, §17.4).
    ReStudyQuotaBlocked {
        /// Why the re-study was blocked.
        reason: String,
    },
}

/// A per-signal baseline + rolling recent window (OP-3).
#[derive(Default)]
struct Series {
    /// The first `baseline_window` sampled values, frozen once full.
    baseline: Vec<f64>,
    /// The trailing `recent_window` sampled values.
    recent: Vec<f64>,
    /// Whether a re-study has already been opened for this signal's drift, so a
    /// sustained drift regime opens one re-study, not one per subsequent run.
    fired: bool,
}

impl Series {
    fn push(&mut self, value: f64, baseline_window: usize, recent_window: usize) {
        if self.baseline.len() < baseline_window {
            self.baseline.push(value);
        } else {
            self.recent.push(value);
            if self.recent.len() > recent_window {
                self.recent.remove(0);
            }
        }
    }

    fn ready(&self, baseline_window: usize, recent_window: usize) -> bool {
        recent_window > 0
            && self.baseline.len() == baseline_window
            && self.recent.len() == recent_window
    }
}

/// The running online operations loop (PRD §15). See the module docs.
pub struct OnlineEvalLoop {
    policy: OpsPolicy,
    series: BTreeMap<DriftKind, Series>,
    feedback: FeedbackStore,
    quotas: QuotaLedger,
    feedback_fired: bool,
    seq: u64,
}

impl OnlineEvalLoop {
    /// Build a loop with the given policy, installing the re-study quota budget.
    pub fn new(policy: OpsPolicy) -> Self {
        let mut quotas = QuotaLedger::new();
        if let Some(limit) = policy.restudy_budget {
            quotas.set_limit(policy.restudy_quota_scope.clone(), limit);
        }
        Self {
            policy,
            series: BTreeMap::new(),
            feedback: FeedbackStore::new(),
            quotas,
            feedback_fired: false,
            seq: 0,
        }
    }

    /// Observe one production run (OP-2/OP-3). Returns the events the run
    /// produced, most-recent last.
    pub fn observe(&mut self, obs: RunObservation) -> Vec<OpsEvent> {
        if !sample_for_eval(&obs.run_id, self.policy.eval_sample_rate) {
            return vec![OpsEvent::NotSampled { run_id: obs.run_id }];
        }
        let mut events = vec![OpsEvent::Sampled {
            run_id: obs.run_id.clone(),
        }];

        // Phase 1: update each signal's windows and collect newly-detected drift.
        // Collecting first ends the `self.series` borrow before we touch the rest
        // of `self` to open incidents and re-studies.
        let (bw, rw, k) = (
            self.policy.baseline_window,
            self.policy.recent_window,
            self.policy.drift_k,
        );
        let mut drifts: Vec<DriftEvidence> = Vec::new();
        for (kind, value) in &obs.metrics {
            let series = self.series.entry(*kind).or_default();
            series.push(*value, bw, rw);
            if series.fired || !series.ready(bw, rw) {
                continue;
            }
            if let Some(ev) = detect_drift(*kind, &series.baseline, &series.recent, k) {
                series.fired = true;
                drifts.push(ev);
            }
        }

        // Phase 2: react to each drift — capture an incident, make it regression
        // coverage (EV-12), and open a bounded re-study under quota (OP-5/OP-7).
        for ev in drifts {
            events.push(OpsEvent::DriftDetected {
                evidence: ev.clone(),
            });
            let incident = self.open_incident(
                format!("{:?} drift (z={:.2})", ev.kind, ev.z_shift),
                vec![obs.trace_ref.clone()],
            );
            events.push(OpsEvent::RegressionCaptured {
                task: incident.to_regression_task(),
            });
            events.push(OpsEvent::IncidentOpened { incident });
            self.push_restudy(
                &mut events,
                ReStudyReason::Drift,
                format!("drift://{:?}", ev.kind),
            );
        }
        events
    }

    /// Ingest one user-feedback item (OP-2/OP-5). Opens a bounded re-study the
    /// first time satisfaction falls below the floor (after `min_feedback`).
    pub fn ingest_feedback(&mut self, feedback: Feedback) -> Vec<OpsEvent> {
        self.feedback.ingest(feedback);
        let mut events = Vec::new();
        if self.feedback_fired || self.feedback.len() < self.policy.min_feedback {
            return events;
        }
        let sat = self.feedback.satisfaction_rate();
        if sat >= self.policy.satisfaction_floor {
            return events;
        }
        self.feedback_fired = true;
        events.push(OpsEvent::SatisfactionBreach { satisfaction: sat });
        let incident = self.open_incident(
            format!(
                "satisfaction {:.2} below floor {:.2}",
                sat, self.policy.satisfaction_floor
            ),
            self.feedback.negative_run_ids(),
        );
        events.push(OpsEvent::RegressionCaptured {
            task: incident.to_regression_task(),
        });
        events.push(OpsEvent::IncidentOpened { incident });
        self.push_restudy(
            &mut events,
            ReStudyReason::Feedback,
            format!("feedback://sat={sat:.2}"),
        );
        events
    }

    /// The current user-satisfaction rate (OP-2).
    pub fn satisfaction_rate(&self) -> f64 {
        self.feedback.satisfaction_rate()
    }

    /// The number of re-studies opened so far (what the quota has charged, OP-7).
    pub fn restudies_opened(&self) -> f64 {
        self.quotas.used(&self.policy.restudy_quota_scope)
    }

    /// Open a bounded re-study if the quota allows, pushing the resulting event
    /// (opened, or a hard-stop block — OP-5/OP-7).
    fn push_restudy(
        &mut self,
        events: &mut Vec<OpsEvent>,
        reason: ReStudyReason,
        evidence: String,
    ) {
        match self
            .quotas
            .charge(self.policy.restudy_quota_scope.clone(), 1.0)
        {
            Ok(()) => {
                self.seq += 1;
                events.push(OpsEvent::ReStudyOpened {
                    restudy: ReStudy {
                        id: format!("rs-{}", self.seq),
                        reason,
                        evidence_ref: evidence,
                    },
                });
            }
            Err(e) => events.push(OpsEvent::ReStudyQuotaBlocked {
                reason: e.to_string(),
            }),
        }
    }

    fn open_incident(&mut self, summary: String, trace_refs: Vec<String>) -> Incident {
        self.seq += 1;
        Incident {
            id: format!("inc-{}", self.seq),
            trace_refs,
            summary,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A policy with small windows for fast, deterministic tests.
    fn policy() -> OpsPolicy {
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

    fn event_names(events: &[OpsEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(|e| match e {
                OpsEvent::Sampled { .. } => "sampled",
                OpsEvent::NotSampled { .. } => "not-sampled",
                OpsEvent::DriftDetected { .. } => "drift",
                OpsEvent::SatisfactionBreach { .. } => "satisfaction-breach",
                OpsEvent::IncidentOpened { .. } => "incident",
                OpsEvent::RegressionCaptured { .. } => "regression",
                OpsEvent::ReStudyOpened { .. } => "restudy",
                OpsEvent::ReStudyQuotaBlocked { .. } => "quota-blocked",
            })
            .collect()
    }

    #[test]
    fn drift_opens_an_incident_regression_and_bounded_restudy() {
        let mut loopy = OnlineEvalLoop::new(policy());
        // Baseline: five steady cost samples (no drift while the baseline fills).
        for i in 0..5 {
            let ev = loopy.observe(RunObservation::one(
                format!("base-{i}"),
                DriftKind::Cost,
                10.0,
                "trace://base",
            ));
            assert_eq!(event_names(&ev), vec!["sampled"], "round {i}");
        }
        // Recent window of three high-cost runs: the third fills the window and
        // trips drift, which opens exactly one incident + regression + re-study.
        let mut fired = None;
        for i in 0..3 {
            let ev = loopy.observe(RunObservation::one(
                format!("hot-{i}"),
                DriftKind::Cost,
                50.0,
                "trace://hot",
            ));
            if ev
                .iter()
                .any(|e| matches!(e, OpsEvent::DriftDetected { .. }))
            {
                fired = Some(ev);
            }
        }
        let ev = fired.expect("drift should fire once the recent window fills");
        assert_eq!(
            event_names(&ev),
            vec!["sampled", "drift", "regression", "incident", "restudy"]
        );
        // The incident became permanent regression coverage (EV-12).
        let regression = ev.iter().find_map(|e| match e {
            OpsEvent::RegressionCaptured { task } => Some(task),
            _ => None,
        });
        let incident = ev.iter().find_map(|e| match e {
            OpsEvent::IncidentOpened { incident } => Some(incident),
            _ => None,
        });
        assert_eq!(
            regression.unwrap().source_incident,
            incident.unwrap().id,
            "regression must derive from the opened incident"
        );
        assert_eq!(loopy.restudies_opened(), 1.0);

        // The signal latches: a further hot run does not open a second re-study.
        let more = loopy.observe(RunObservation::one(
            "hot-3",
            DriftKind::Cost,
            50.0,
            "trace://hot",
        ));
        assert!(!more
            .iter()
            .any(|e| matches!(e, OpsEvent::ReStudyOpened { .. })));
        assert_eq!(loopy.restudies_opened(), 1.0);
    }

    #[test]
    fn steady_signal_never_opens_a_restudy() {
        let mut loopy = OnlineEvalLoop::new(policy());
        for i in 0..20 {
            // Tiny noise, well within k sigma.
            let v = 10.0 + (i % 2) as f64 * 0.1;
            let ev = loopy.observe(RunObservation::one(
                format!("r-{i}"),
                DriftKind::Quality,
                v,
                "trace://r",
            ));
            assert!(!ev
                .iter()
                .any(|e| matches!(e, OpsEvent::DriftDetected { .. })));
        }
        assert_eq!(loopy.restudies_opened(), 0.0);
    }

    #[test]
    fn unsampled_runs_do_not_feed_drift() {
        // At rate 0 nothing is sampled, so no window ever fills and no drift fires.
        let mut p = policy();
        p.eval_sample_rate = 0.0;
        let mut loopy = OnlineEvalLoop::new(p);
        for i in 0..30 {
            let v = if i < 5 { 10.0 } else { 90.0 };
            let ev = loopy.observe(RunObservation::one(
                format!("r-{i}"),
                DriftKind::Cost,
                v,
                "t",
            ));
            assert_eq!(event_names(&ev), vec!["not-sampled"]);
        }
        assert_eq!(loopy.restudies_opened(), 0.0);
    }

    #[test]
    fn feedback_below_floor_opens_one_restudy_after_minimum() {
        let mut loopy = OnlineEvalLoop::new(policy());
        // Three negatives before the minimum (4) is reached: no trigger yet.
        for i in 0..3 {
            let ev = loopy.ingest_feedback(Feedback {
                run_id: format!("n-{i}"),
                positive: false,
                note: None,
            });
            assert!(ev.is_empty(), "below min_feedback should not trigger");
        }
        // The fourth item crosses the minimum with satisfaction 0.0 < floor.
        let ev = loopy.ingest_feedback(Feedback {
            run_id: "n-3".into(),
            positive: false,
            note: None,
        });
        assert_eq!(
            event_names(&ev),
            vec!["satisfaction-breach", "regression", "incident", "restudy"]
        );
        // It latches: more negative feedback does not open another re-study.
        let again = loopy.ingest_feedback(Feedback {
            run_id: "n-4".into(),
            positive: false,
            note: None,
        });
        assert!(again.is_empty());
        assert_eq!(loopy.restudies_opened(), 1.0);
    }

    #[test]
    fn restudy_quota_is_a_hard_stop() {
        // Budget of one re-study: the first drift opens it, the second is blocked.
        let mut p = policy();
        p.restudy_budget = Some(1.0);
        let mut loopy = OnlineEvalLoop::new(p);

        // Fill a shared baseline on two signals, then drift both.
        for i in 0..5 {
            loopy.observe(RunObservation {
                run_id: format!("b-{i}"),
                metrics: BTreeMap::from([(DriftKind::Cost, 10.0), (DriftKind::Quality, 10.0)]),
                trace_ref: "t".into(),
            });
        }
        let mut saw_open = false;
        let mut saw_blocked = false;
        for i in 0..3 {
            let ev = loopy.observe(RunObservation {
                run_id: format!("h-{i}"),
                metrics: BTreeMap::from([(DriftKind::Cost, 99.0), (DriftKind::Quality, 99.0)]),
                trace_ref: "t".into(),
            });
            saw_open |= ev
                .iter()
                .any(|e| matches!(e, OpsEvent::ReStudyOpened { .. }));
            saw_blocked |= ev
                .iter()
                .any(|e| matches!(e, OpsEvent::ReStudyQuotaBlocked { .. }));
        }
        assert!(saw_open, "the first drift should open a re-study");
        assert!(
            saw_blocked,
            "the second drift should hit the quota hard stop"
        );
        // Only the one budgeted re-study was charged.
        assert_eq!(loopy.restudies_opened(), 1.0);
    }

    #[test]
    fn event_stream_serializes_as_a_tagged_log() {
        let ev = OpsEvent::ReStudyOpened {
            restudy: ReStudy {
                id: "rs-1".into(),
                reason: ReStudyReason::Drift,
                evidence_ref: "drift://Cost".into(),
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains(r#""event":"re-study-opened""#), "{json}");
        let back: OpsEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }
}
