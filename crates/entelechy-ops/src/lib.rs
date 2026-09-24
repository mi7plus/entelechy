//! Operations and evolution: drift, incidents, re-studies and quotas.
//!
//! PRD v11 references: section 15 (operations & evolution, OP-1..OP-7), EV-12
//! (incidents become regression coverage), 14.3/21 (re-study outputs wait at the
//! release gate; no auto-promotion), 17.4 (quotas enforced outside model output).
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Deterministically decide whether a run is sampled for online evaluation at the
/// given rate (PRD OP-2: online sampled evaluation). Uses a hash of the run id so
/// the decision is stable and reproducible for a given id.
pub fn sample_for_eval(run_id: &str, rate: f64) -> bool {
    if rate <= 0.0 {
        return false;
    }
    if rate >= 1.0 {
        return true;
    }
    // FNV-1a over the run id -> a fraction in [0,1).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in run_id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    let fraction = (h % 1_000_000) as f64 / 1_000_000.0;
    fraction < rate
}

/// A unit of ingested user feedback for a run (PRD OP-2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feedback {
    /// The run the feedback concerns.
    pub run_id: String,
    /// Whether the outcome was satisfactory.
    pub positive: bool,
    /// Optional free-text note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A store of ingested user feedback (PRD OP-2). Feeds regression coverage and
/// re-study triggers; it does not itself promote anything.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FeedbackStore {
    items: Vec<Feedback>,
}

impl FeedbackStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }
    /// Ingest a feedback item.
    pub fn ingest(&mut self, feedback: Feedback) {
        self.items.push(feedback);
    }
    /// Number of feedback items.
    pub fn len(&self) -> usize {
        self.items.len()
    }
    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// Fraction of positive feedback in `[0,1]` (1.0 when empty).
    pub fn satisfaction_rate(&self) -> f64 {
        if self.items.is_empty() {
            return 1.0;
        }
        let pos = self.items.iter().filter(|f| f.positive).count();
        pos as f64 / self.items.len() as f64
    }
    /// Run ids with negative feedback — candidates for incident/regression capture.
    pub fn negative_run_ids(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|f| !f.positive)
            .map(|f| f.run_id.clone())
            .collect()
    }
}

/// The kind of signal a drift event concerns (PRD OP-3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DriftKind {
    /// Input distribution drift.
    Input,
    /// Tool behavior drift.
    Tool,
    /// Model behavior drift (see also provider fingerprint, PRD 5.10).
    Model,
    /// Cost drift.
    Cost,
    /// Quality drift.
    Quality,
}

/// Evidence for a detected drift (PRD OP-3: detect drift with evidence).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DriftEvidence {
    /// The signal kind.
    pub kind: DriftKind,
    /// Baseline mean.
    pub baseline_mean: f64,
    /// Recent-window mean.
    pub window_mean: f64,
    /// Shift in baseline-standard-deviation units (the drift statistic).
    pub z_shift: f64,
}

/// Detect drift by comparing a recent window's mean to the baseline, measured in
/// baseline standard deviations (PRD OP-3). Returns evidence when the absolute
/// shift exceeds `k` sigma. A zero-variance baseline drifts on any change.
pub fn detect_drift(
    kind: DriftKind,
    baseline: &[f64],
    window: &[f64],
    k: f64,
) -> Option<DriftEvidence> {
    if baseline.is_empty() || window.is_empty() {
        return None;
    }
    let bmean = mean(baseline);
    let wmean = mean(window);
    let bstd = stddev(baseline, bmean);
    let z_shift = if bstd > 0.0 {
        (wmean - bmean) / bstd
    } else if (wmean - bmean).abs() > f64::EPSILON {
        f64::INFINITY
    } else {
        0.0
    };
    if z_shift.abs() > k {
        Some(DriftEvidence {
            kind,
            baseline_mean: bmean,
            window_mean: wmean,
            z_shift,
        })
    } else {
        None
    }
}

fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len() as f64
}

fn stddev(xs: &[f64], mean: f64) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (xs.len() as f64 - 1.0);
    var.sqrt()
}

/// A production incident captured as a trace bundle (PRD OP-4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Incident {
    /// Incident id.
    pub id: String,
    /// References to the trace bundle(s) evidencing the failure.
    pub trace_refs: Vec<String>,
    /// Short summary.
    pub summary: String,
}

/// A permanent regression task derived from an incident (PRD OP-4, EV-12).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegressionTask {
    /// Regression task id.
    pub id: String,
    /// The incident it was derived from.
    pub source_incident: String,
}

impl Incident {
    /// Convert an incident into permanent regression coverage (OP-4, EV-12).
    /// The regression task does not retroactively become independent holdout
    /// evidence (PRD 9.5).
    pub fn to_regression_task(&self) -> RegressionTask {
        RegressionTask {
            id: format!("regression-from-{}", self.id),
            source_incident: self.id.clone(),
        }
    }
}

/// Why a bounded re-study was opened (PRD OP-5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReStudyReason {
    /// Detected drift.
    Drift,
    /// A deprecation (e.g. model retirement).
    Deprecation,
    /// User feedback.
    Feedback,
    /// An incident.
    Incident,
}

/// A bounded re-study opened by operations (PRD OP-5). Its outputs wait at the
/// release gate — it never auto-promotes (PRD non-goal: autonomous production
/// promotion by default).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReStudy {
    /// Re-study id.
    pub id: String,
    /// Why it was opened.
    pub reason: ReStudyReason,
    /// Evidence reference (drift evidence, incident id, feedback ref).
    pub evidence_ref: String,
}

impl ReStudy {
    /// Re-studies never auto-promote; their candidates wait at the release gate
    /// (PRD OP-5, 14.3).
    pub const fn auto_promotes(&self) -> bool {
        false
    }
}

/// The scope a quota applies to (PRD OP-7).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuotaScope {
    /// A single run.
    Run(String),
    /// The whole system.
    System,
    /// A team.
    Team(String),
    /// A study.
    Study(String),
}

/// Quota exceeded error (a hard stop — PRD OP-7, 17.4).
#[derive(Debug, thiserror::Error, PartialEq)]
#[error("quota exceeded for {scope:?}: {used}+{amount} > {limit}")]
pub struct QuotaExceeded {
    /// The scope whose quota was exceeded.
    pub scope: QuotaScope,
    /// Amount already used.
    pub used: f64,
    /// Amount requested.
    pub amount: f64,
    /// The limit.
    pub limit: f64,
}

/// Per-scope quotas with hard stops, enforced outside model output (PRD OP-7,
/// 17.4).
#[derive(Clone, Debug, Default)]
pub struct QuotaLedger {
    limits: BTreeMap<QuotaScope, f64>,
    used: BTreeMap<QuotaScope, f64>,
}

impl QuotaLedger {
    /// Create an empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a limit for a scope.
    pub fn set_limit(&mut self, scope: QuotaScope, limit: f64) {
        self.limits.insert(scope, limit);
    }

    /// Charge an amount against a scope, hard-stopping if it would exceed the
    /// limit (PRD OP-7). Scopes without a limit are unbounded.
    pub fn charge(&mut self, scope: QuotaScope, amount: f64) -> Result<(), QuotaExceeded> {
        let used = *self.used.get(&scope).unwrap_or(&0.0);
        if let Some(&limit) = self.limits.get(&scope) {
            if used + amount > limit {
                return Err(QuotaExceeded {
                    scope,
                    used,
                    amount,
                    limit,
                });
            }
        }
        self.used.insert(scope, used + amount);
        Ok(())
    }

    /// Amount used against a scope.
    pub fn used(&self, scope: &QuotaScope) -> f64 {
        *self.used.get(scope).unwrap_or(&0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_mean_shift_beyond_k_sigma() {
        let baseline = vec![10.0, 11.0, 9.0, 10.0, 10.0];
        let drifted = vec![20.0, 21.0, 19.0];
        let ev = detect_drift(DriftKind::Cost, &baseline, &drifted, 3.0).unwrap();
        assert!(ev.z_shift > 3.0);
        // No drift for a similar window.
        let steady = vec![10.0, 10.5, 9.5];
        assert!(detect_drift(DriftKind::Cost, &baseline, &steady, 3.0).is_none());
    }

    #[test]
    fn incident_becomes_regression() {
        let inc = Incident {
            id: "inc-42".into(),
            trace_refs: vec!["trace://abc".into()],
            summary: "refund leaked".into(),
        };
        let r = inc.to_regression_task();
        assert_eq!(r.source_incident, "inc-42");
        assert_eq!(r.id, "regression-from-inc-42");
    }

    #[test]
    fn restudy_never_auto_promotes() {
        let rs = ReStudy {
            id: "rs-1".into(),
            reason: ReStudyReason::Drift,
            evidence_ref: "drift://cost".into(),
        };
        assert!(!rs.auto_promotes());
    }

    #[test]
    fn online_sampling_is_deterministic_and_rate_sensitive() {
        // Deterministic for a given id.
        assert_eq!(sample_for_eval("run-1", 0.5), sample_for_eval("run-1", 0.5));
        // Extremes.
        assert!(!sample_for_eval("run-1", 0.0));
        assert!(sample_for_eval("run-1", 1.0));
        // Over many ids, a ~50% rate samples a middling fraction.
        let sampled = (0..1000)
            .filter(|i| sample_for_eval(&format!("run-{i}"), 0.5))
            .count();
        assert!((400..600).contains(&sampled), "sampled={sampled}");
    }

    #[test]
    fn feedback_store_aggregates() {
        let mut fb = FeedbackStore::new();
        fb.ingest(Feedback {
            run_id: "a".into(),
            positive: true,
            note: None,
        });
        fb.ingest(Feedback {
            run_id: "b".into(),
            positive: false,
            note: Some("wrong answer".into()),
        });
        assert!((fb.satisfaction_rate() - 0.5).abs() < 1e-9);
        assert_eq!(fb.negative_run_ids(), vec!["b".to_string()]);
    }

    #[test]
    fn quota_hard_stop() {
        let mut q = QuotaLedger::new();
        q.set_limit(QuotaScope::Study("s1".into()), 100.0);
        assert!(q.charge(QuotaScope::Study("s1".into()), 60.0).is_ok());
        // Would exceed → hard stop.
        assert!(q.charge(QuotaScope::Study("s1".into()), 50.0).is_err());
        assert_eq!(q.used(&QuotaScope::Study("s1".into())), 60.0);
        // Unlimited scope charges freely.
        assert!(q.charge(QuotaScope::System, 1e9).is_ok());
    }
}
