//! Failure analyzer (PRD 10.2, Q3).
//!
//! Clusters failed traces by structural signature, assigns one or more failure
//! classes with calibrated confidence and evidence, distinguishes system failure
//! from evaluation failure, and maintains an unresolved-cause state rather than
//! forcing a confident diagnosis (PRD 10.2). Search budget is allocated across
//! clusters in proportion to probability × expected impact, with a fraction
//! reserved for high-entropy clusters (PRD Q3).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ontology::{class, FailureClass, Family};

/// The confidence below which a diagnosis is uncertain / diagnostic-only (Q3).
pub const DIAGNOSTIC_CONFIDENCE: f64 = 0.6;

/// Fraction of the search budget reserved for high-entropy clusters (Q3).
pub const HIGH_ENTROPY_RESERVE: f64 = 0.20;

/// A typed symptom observed on a failed task (a structural/semantic feature).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Symptom {
    /// The system produced an empty or missing required output.
    EmptyOutput,
    /// A claim was unsupported by evidence (grounding).
    UnsupportedClaim,
    /// The wrong tool was selected.
    WrongTool,
    /// A tool call errored.
    ToolError,
    /// The run hit a timeout.
    Timeout,
    /// A policy/authority check denied an action.
    PolicyDenied,
    /// The context window overflowed.
    ContextOverflow,
    /// Retrieval failed to surface the needed document.
    RetrievalMiss,
    /// Judges disagreed (evaluation-side).
    JudgeDisagreement,
    /// Uncharacterized failure.
    Unknown,
}

impl Symptom {
    /// The failure class this symptom most directly indicates (PRD 10.1).
    /// `Unknown` maps to nothing, contributing to cluster entropy.
    pub fn indicated_class(self) -> Option<FailureClass> {
        Some(match self {
            Symptom::EmptyOutput => class(Family::Reasoning, "synthesis"),
            Symptom::UnsupportedClaim => class(Family::Reasoning, "verification"),
            Symptom::WrongTool => class(Family::Tool, "selection"),
            Symptom::ToolError => class(Family::Tool, "execution"),
            Symptom::Timeout => class(Family::Model, "instability"),
            Symptom::PolicyDenied => class(Family::Policy, "unauthorized_action"),
            Symptom::ContextOverflow => class(Family::Knowledge, "context_overflow"),
            Symptom::RetrievalMiss => class(Family::Knowledge, "retrieval_miss"),
            Symptom::JudgeDisagreement => class(Family::Evaluation, "judge_disagreement"),
            Symptom::Unknown => return None,
        })
    }
}

/// A single failed-task observation fed to the analyzer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FailureObservation {
    /// Task id that failed.
    pub task_id: String,
    /// Structural cluster signature (e.g. failing node path + error kind).
    pub signature: String,
    /// The observed symptom.
    pub symptom: Symptom,
    /// Whether this is an evaluation failure rather than a system failure (10.2).
    pub is_evaluation_failure: bool,
}

/// A weighted class assignment within a cluster.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClassProbability {
    /// The class id (e.g. `reasoning.verification`).
    pub class_id: String,
    /// Calibrated probability in `[0,1]`.
    pub probability: f64,
    /// The complexity levels this class makes eligible (PRD 11.4).
    pub eligible_levels: Vec<u8>,
}

/// A cluster of related failures (PRD 10.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FailureCluster {
    /// Cluster id (its signature).
    pub id: String,
    /// Member task ids (evidence).
    pub members: Vec<String>,
    /// Class distribution, highest probability first.
    pub class_distribution: Vec<ClassProbability>,
    /// Shannon entropy (bits) of the distribution.
    pub entropy: f64,
    /// Whether this cluster is predominantly an evaluation failure (10.2).
    pub is_evaluation_failure: bool,
}

impl FailureCluster {
    /// The top class and its probability, if any.
    pub fn top(&self) -> Option<&ClassProbability> {
        self.class_distribution.first()
    }

    /// Whether the diagnosis is confident enough to act on directly (Q3). Below
    /// the threshold it must run as a diagnostic experiment and may end
    /// inconclusive (PRD 10.2 unresolved-cause state).
    pub fn is_confident(&self) -> bool {
        self.top()
            .is_some_and(|c| c.probability >= DIAGNOSTIC_CONFIDENCE)
    }

    /// Whether this is a high-entropy (ambiguous) cluster reserved a budget share.
    pub fn is_high_entropy(&self) -> bool {
        self.entropy >= 1.0
    }
}

/// Cluster failed observations and assign calibrated class distributions.
pub fn analyze(observations: &[FailureObservation]) -> Vec<FailureCluster> {
    // Group by structural signature (PRD 10.2 structural clustering).
    let mut groups: BTreeMap<String, Vec<&FailureObservation>> = BTreeMap::new();
    for obs in observations {
        groups.entry(obs.signature.clone()).or_default().push(obs);
    }

    let mut clusters: Vec<FailureCluster> = groups
        .into_iter()
        .map(|(sig, members)| {
            // Count indicated classes; unknown symptoms add mass to a residual
            // "unresolved" bucket that raises entropy.
            let mut counts: BTreeMap<String, (f64, Vec<u8>)> = BTreeMap::new();
            let mut unresolved = 0.0;
            for m in &members {
                match m.symptom.indicated_class() {
                    Some(c) => {
                        let entry = counts
                            .entry(c.id())
                            .or_insert((0.0, c.eligible_levels().to_vec()));
                        entry.0 += 1.0;
                    }
                    None => unresolved += 1.0,
                }
            }
            let total = members.len() as f64;
            let mut dist: Vec<ClassProbability> = counts
                .into_iter()
                .map(|(class_id, (n, levels))| ClassProbability {
                    class_id,
                    probability: n / total,
                    eligible_levels: levels,
                })
                .collect();
            if unresolved > 0.0 {
                dist.push(ClassProbability {
                    class_id: "unresolved".into(),
                    probability: unresolved / total,
                    eligible_levels: vec![],
                });
            }
            dist.sort_by(|a, b| b.probability.partial_cmp(&a.probability).unwrap());
            let entropy = shannon_entropy(dist.iter().map(|c| c.probability));
            let eval_failures = members.iter().filter(|m| m.is_evaluation_failure).count();
            let is_eval = eval_failures * 2 > members.len();
            FailureCluster {
                id: sig,
                members: members.iter().map(|m| m.task_id.clone()).collect(),
                class_distribution: dist,
                entropy,
                is_evaluation_failure: is_eval,
            }
        })
        .collect();

    // Largest clusters first (impact ordering).
    clusters.sort_by(|a, b| b.members.len().cmp(&a.members.len()).then(a.id.cmp(&b.id)));
    clusters
}

/// Allocate a search budget across clusters (PRD Q3): reserve a fixed fraction
/// for high-entropy clusters, then distribute the rest proportional to
/// probability × expected impact (member count). Meta-defect clusters
/// (specification/evaluation) receive no design-search budget.
pub fn allocate_budget(clusters: &[FailureCluster], total: f64) -> Vec<(String, f64)> {
    // Actionable clusters only: skip pure evaluation-failure clusters.
    let actionable: Vec<&FailureCluster> = clusters
        .iter()
        .filter(|c| !c.is_evaluation_failure)
        .collect();
    if actionable.is_empty() {
        return Vec::new();
    }

    let reserve = total * HIGH_ENTROPY_RESERVE;
    let main = total - reserve;

    // Proportional weights = top probability × impact (member count).
    let weights: Vec<f64> = actionable
        .iter()
        .map(|c| c.top().map(|t| t.probability).unwrap_or(0.0) * c.members.len() as f64)
        .collect();
    let weight_sum: f64 = weights.iter().sum();

    // High-entropy reserve split equally among high-entropy actionable clusters.
    let high: Vec<usize> = actionable
        .iter()
        .enumerate()
        .filter(|(_, c)| c.is_high_entropy())
        .map(|(i, _)| i)
        .collect();
    let per_high = if high.is_empty() {
        0.0
    } else {
        reserve / high.len() as f64
    };

    actionable
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let proportional = if weight_sum > 0.0 {
                main * weights[i] / weight_sum
            } else {
                0.0
            };
            let reserved = if high.contains(&i) { per_high } else { 0.0 };
            (c.id.clone(), proportional + reserved)
        })
        .collect()
}

fn shannon_entropy(probs: impl Iterator<Item = f64>) -> f64 {
    probs.filter(|p| *p > 0.0).map(|p| -p * p.log2()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(task: &str, sig: &str, s: Symptom, eval: bool) -> FailureObservation {
        FailureObservation {
            task_id: task.into(),
            signature: sig.into(),
            symptom: s,
            is_evaluation_failure: eval,
        }
    }

    #[test]
    fn homogeneous_cluster_is_confident() {
        let obns = vec![
            obs("t1", "draft:empty", Symptom::EmptyOutput, false),
            obs("t2", "draft:empty", Symptom::EmptyOutput, false),
            obs("t3", "draft:empty", Symptom::EmptyOutput, false),
        ];
        let clusters = analyze(&obns);
        assert_eq!(clusters.len(), 1);
        let c = &clusters[0];
        assert!(c.is_confident());
        assert_eq!(c.top().unwrap().class_id, "reasoning.synthesis");
        assert!(!c.is_high_entropy());
    }

    #[test]
    fn mixed_cluster_is_uncertain_and_high_entropy() {
        let obns = vec![
            obs("t1", "node7", Symptom::WrongTool, false),
            obs("t2", "node7", Symptom::RetrievalMiss, false),
            obs("t3", "node7", Symptom::Unknown, false),
            obs("t4", "node7", Symptom::Timeout, false),
        ];
        let clusters = analyze(&obns);
        let c = &clusters[0];
        assert!(!c.is_confident(), "dist={:?}", c.class_distribution);
        assert!(c.is_high_entropy());
    }

    #[test]
    fn budget_reserves_for_high_entropy_and_skips_eval() {
        let obns = vec![
            // Confident actionable cluster (big impact).
            obs("a1", "sig_a", Symptom::UnsupportedClaim, false),
            obs("a2", "sig_a", Symptom::UnsupportedClaim, false),
            obs("a3", "sig_a", Symptom::UnsupportedClaim, false),
            // High-entropy actionable cluster.
            obs("b1", "sig_b", Symptom::WrongTool, false),
            obs("b2", "sig_b", Symptom::RetrievalMiss, false),
            obs("b3", "sig_b", Symptom::Unknown, false),
            // Evaluation-failure cluster (should get nothing).
            obs("e1", "sig_e", Symptom::JudgeDisagreement, true),
            obs("e2", "sig_e", Symptom::JudgeDisagreement, true),
        ];
        let clusters = analyze(&obns);
        let alloc = allocate_budget(&clusters, 100.0);
        // Only the two actionable clusters get budget.
        assert_eq!(alloc.len(), 2);
        let total: f64 = alloc.iter().map(|(_, b)| b).sum();
        assert!((total - 100.0).abs() < 1e-6, "alloc={alloc:?}");
        // The high-entropy cluster (sig_b) receives its reserved share.
        let b = alloc.iter().find(|(id, _)| id == "sig_b").unwrap().1;
        assert!(b >= 100.0 * HIGH_ENTROPY_RESERVE);
    }
}
