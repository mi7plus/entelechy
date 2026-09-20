//! Judge ensembles, calibration and bias measurement (PRD EV-7, EV-16, EV-17).
//!
//! Judge ensembles are calibrated against human-labeled samples and report
//! agreement (EV-7). The failure analyzer is evaluated against human-labeled
//! failures with per-class precision and recall (EV-16). Different model families
//! are used for design, analysis and judging where available, and self-preference
//! bias is measured (EV-17). These are LLM-free statistics over verdicts.

use serde::{Deserialize, Serialize};

/// The aggregate verdict of an ensemble of binary judges on one item (EV-7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnsembleVerdict {
    /// Majority decision (pass/fail).
    pub majority: bool,
    /// Fraction of judges agreeing with the majority, in `[0.5, 1.0]`.
    pub agreement: f64,
    /// Number of judges.
    pub n: usize,
}

/// Aggregate a set of binary judge verdicts into a majority verdict with an
/// agreement score (EV-7). Ties resolve to `false` (fail-closed).
pub fn ensemble(verdicts: &[bool]) -> EnsembleVerdict {
    let n = verdicts.len();
    if n == 0 {
        return EnsembleVerdict { majority: false, agreement: 0.0, n: 0 };
    }
    let yes = verdicts.iter().filter(|v| **v).count();
    let no = n - yes;
    let majority = yes > no;
    let agreement = yes.max(no) as f64 / n as f64;
    EnsembleVerdict { majority, agreement, n }
}

/// Judge–human agreement rate: fraction of items where the judge matches the
/// human label (EV-7). Vectors must be equal length.
pub fn agreement_rate(judge: &[bool], human: &[bool]) -> f64 {
    assert_eq!(judge.len(), human.len(), "agreement requires equal-length vectors");
    if judge.is_empty() {
        return 1.0;
    }
    let matches = judge.iter().zip(human).filter(|(a, b)| a == b).count();
    matches as f64 / judge.len() as f64
}

/// Cohen's kappa between judge and human labels: agreement corrected for chance
/// (EV-7 calibration). 1.0 is perfect, 0.0 is chance-level, negative is worse
/// than chance.
pub fn cohens_kappa(judge: &[bool], human: &[bool]) -> f64 {
    assert_eq!(judge.len(), human.len(), "kappa requires equal-length vectors");
    let n = judge.len() as f64;
    if n == 0.0 {
        return 1.0;
    }
    let po = agreement_rate(judge, human);
    let pj = judge.iter().filter(|v| **v).count() as f64 / n;
    let ph = human.iter().filter(|v| **v).count() as f64 / n;
    let pe = pj * ph + (1.0 - pj) * (1.0 - ph);
    if (1.0 - pe).abs() < f64::EPSILON {
        // Both raters constant and identical -> perfect; else undefined -> 0.
        return if po >= 1.0 { 1.0 } else { 0.0 };
    }
    (po - pe) / (1.0 - pe)
}

/// Per-class precision and recall of a binary predictor against ground truth
/// (EV-16: evaluate the failure analyzer per class). Returns `(precision, recall)`.
pub fn precision_recall(predicted: &[bool], actual: &[bool]) -> (f64, f64) {
    assert_eq!(predicted.len(), actual.len(), "precision/recall need equal-length vectors");
    let mut tp = 0.0;
    let mut fp = 0.0;
    let mut fn_ = 0.0;
    for (p, a) in predicted.iter().zip(actual) {
        match (p, a) {
            (true, true) => tp += 1.0,
            (true, false) => fp += 1.0,
            (false, true) => fn_ += 1.0,
            (false, false) => {}
        }
    }
    let precision = if tp + fp == 0.0 { 1.0 } else { tp / (tp + fp) };
    let recall = if tp + fn_ == 0.0 { 1.0 } else { tp / (tp + fn_) };
    (precision, recall)
}

/// Self-preference bias (EV-17): the mean score a judge gives outputs from its
/// own model family minus the mean it gives other families' outputs. Positive
/// means the judge favors its own family; near zero is unbiased.
pub fn self_preference_bias(own_family_scores: &[f64], other_family_scores: &[f64]) -> f64 {
    let mean = |xs: &[f64]| {
        if xs.is_empty() {
            0.0
        } else {
            xs.iter().sum::<f64>() / xs.len() as f64
        }
    };
    mean(own_family_scores) - mean(other_family_scores)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensemble_majority_and_agreement() {
        let v = ensemble(&[true, true, false]);
        assert!(v.majority);
        assert!((v.agreement - 2.0 / 3.0).abs() < 1e-9);
        // Tie fails closed.
        let t = ensemble(&[true, false]);
        assert!(!t.majority);
    }

    #[test]
    fn agreement_and_kappa() {
        let judge = vec![true, true, false, false];
        let human = vec![true, false, false, false];
        assert!((agreement_rate(&judge, &human) - 0.75).abs() < 1e-9);
        let k = cohens_kappa(&judge, &human);
        assert!(k > 0.0 && k < 1.0, "kappa={k}");
        // Perfect agreement -> kappa 1.
        assert!((cohens_kappa(&human, &human) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn precision_recall_basic() {
        // predicted refund-violation, actual refund-violation.
        let predicted = vec![true, true, false, false];
        let actual = vec![true, false, true, false];
        let (p, r) = precision_recall(&predicted, &actual);
        assert!((p - 0.5).abs() < 1e-9); // 1 TP, 1 FP
        assert!((r - 0.5).abs() < 1e-9); // 1 TP, 1 FN
    }

    #[test]
    fn self_preference_bias_detected() {
        let own = vec![0.9, 0.85, 0.95];
        let other = vec![0.6, 0.65, 0.55];
        assert!(self_preference_bias(&own, &other) > 0.2);
        // Unbiased judge.
        assert!(self_preference_bias(&own, &own).abs() < 1e-9);
    }
}
