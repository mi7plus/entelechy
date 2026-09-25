//! DesignHypothesis: evidence-backed proposed intervention (PRD 5.2, 11.2).
//!
//! Every design change is a hypothesis (PRD principle 5): it names the failure
//! class, cause, patch, expected effect and cost, and carries an experiment and a
//! result. A hypothesis whose class confidence is below 0.6 runs as a diagnostic
//! experiment and may end inconclusive (PRD Q3).

use serde::{Deserialize, Serialize};

use crate::edit::EditOp;

/// The lifecycle result of a hypothesis (PRD 11.2 result field).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HypothesisResult {
    /// Not yet evaluated.
    Pending,
    /// Confirmed and retained.
    Accepted,
    /// Falsified and reverted.
    Rejected,
    /// Neither confirmed nor falsified (PRD 10.2 unresolved-cause state).
    Inconclusive,
    /// Replaced by a later hypothesis.
    Superseded,
}

/// The confidence threshold below which a hypothesis is diagnostic-only (Q3).
pub const DIAGNOSTIC_CONFIDENCE: f64 = 0.6;

/// An evidence-backed proposed intervention (PRD 11.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DesignHypothesis {
    /// Stable identifier, e.g. `H-42`.
    pub id: String,
    /// Failure clusters, traces, eval deltas or capability gaps motivating it.
    pub evidence: String,
    /// The failure ontology class being addressed (PRD 10.1).
    pub failure_class: String,
    /// The causal hypothesis.
    pub suspected_cause: String,
    /// Calibrated confidence in the cause, in `[0,1]` (PRD Q3).
    pub cause_confidence: f64,
    /// The typed IR/resource change (PRD 11.5).
    pub patch: Vec<EditOp>,
    /// Metric or failure behavior expected to improve.
    pub expected_effect: String,
    /// Cost, latency, complexity or risk expected to change.
    pub expected_tradeoff: String,
    /// Tasks, repetitions and comparison method for the experiment.
    pub experiment: String,
    /// The lifecycle result.
    pub result: HypothesisResult,
}

impl DesignHypothesis {
    /// A multi-agent decomposition hypothesis (PRD 11.4 level 4/5): the single
    /// agent is failing a `reasoning.decomposition` class the higher levels
    /// address, so split it into parallel specialists ([`EditOp::SplitParallel`])
    /// or delegate scoped sub-agents. This is the evidence-gated entry point to the
    /// multi-agent operators — it should be formed only after the failure analyzer
    /// attributes failures to decomposition/coordination and the Architecture
    /// Explorer unlocks the level (PRD 11.4, Q4).
    pub fn decomposition(
        id: impl Into<String>,
        evidence: impl Into<String>,
        confidence: f64,
        patch: Vec<EditOp>,
    ) -> Self {
        Self {
            id: id.into(),
            evidence: evidence.into(),
            failure_class: "reasoning.decomposition".into(),
            suspected_cause: "a single agent cannot decompose the task; specialized \
                              sub-agents should each handle a part"
                .into(),
            cause_confidence: confidence,
            patch,
            expected_effect: "raise task success on tasks needing decomposition".into(),
            expected_tradeoff: "higher cost/latency and coordination complexity".into(),
            experiment: "paired eval vs the single-agent parent on tune, confirm on validation"
                .into(),
            result: HypothesisResult::Pending,
        }
    }

    /// Whether this hypothesis is diagnostic-only: its cause confidence is below
    /// the threshold, so it may only end accepted or inconclusive (Q3).
    pub fn is_diagnostic(&self) -> bool {
        self.cause_confidence < DIAGNOSTIC_CONFIDENCE
    }

    /// Record the experiment result. A diagnostic hypothesis is never allowed to
    /// end as a confident `Rejected`; it becomes `Inconclusive` instead (Q3, 10.2).
    pub fn resolve(&mut self, outcome: HypothesisResult) {
        self.result = if self.is_diagnostic() && outcome == HypothesisResult::Rejected {
            HypothesisResult::Inconclusive
        } else {
            outcome
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hyp(conf: f64) -> DesignHypothesis {
        DesignHypothesis {
            id: "H-1".into(),
            evidence: "cluster FC-1: unsupported claims in 27/400 tune tasks".into(),
            failure_class: "reasoning.verification".into(),
            suspected_cause: "no post-synthesis verification".into(),
            cause_confidence: conf,
            patch: vec![EditOp::AddVerify {
                id: "v".into(),
                checker: "supported".into(),
            }],
            expected_effect: "reduce unsupported-claim failures".into(),
            expected_tradeoff: "small cost/latency increase".into(),
            experiment: "paired eval vs parent on tune, confirm on validation".into(),
            result: HypothesisResult::Pending,
        }
    }

    #[test]
    fn confident_hypothesis_can_be_rejected() {
        let mut h = hyp(0.8);
        assert!(!h.is_diagnostic());
        h.resolve(HypothesisResult::Rejected);
        assert_eq!(h.result, HypothesisResult::Rejected);
    }

    #[test]
    fn diagnostic_hypothesis_never_confidently_rejects() {
        let mut h = hyp(0.4);
        assert!(h.is_diagnostic());
        h.resolve(HypothesisResult::Rejected);
        assert_eq!(h.result, HypothesisResult::Inconclusive);
    }

    #[test]
    fn decomposition_hypothesis_targets_the_multi_agent_class() {
        let h = DesignHypothesis::decomposition(
            "H-9",
            "cluster FC-3: multi-part tickets fail as a single agent",
            0.7,
            vec![EditOp::SplitParallel {
                node: "agent".into(),
                agents: vec![
                    crate::AgentSpec {
                        id: "triage".into(),
                        prompt_template: "triage: {input}".into(),
                    },
                    crate::AgentSpec {
                        id: "resolve".into(),
                        prompt_template: "resolve: {input}".into(),
                    },
                ],
            }],
        );
        assert_eq!(h.failure_class, "reasoning.decomposition");
        assert!(!h.is_diagnostic());
        assert_eq!(h.patch.len(), 1);
    }
}
