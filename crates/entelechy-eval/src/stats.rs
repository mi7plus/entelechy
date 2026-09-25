//! Statistical validity: paired comparisons, bootstrap intervals, power and
//! negative-goal upper bounds (PRD 9.4, EV-9, 5.6).
//!
//! Tasks — not repeated runs — are the unit of independence (PRD 9.4). All
//! randomness here is a seeded, deterministic PRNG so results are reproducible
//! (PRD 18 replay determinism) without an external dependency.

use serde::{Deserialize, Serialize};

/// A tiny deterministic PRNG (xorshift64*). Not cryptographic; used only for
/// reproducible bootstrap resampling.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // Avoid the zero state.
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform index in `[0, n)`.
    fn index(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// Result of a paired success-rate comparison of two designs (PRD 9.4, EV-9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    /// Number of paired tasks (the unit of independence).
    pub n: usize,
    /// Estimated improvement in success rate (candidate − baseline), fraction.
    pub delta: f64,
    /// Lower bound of the 95% bootstrap interval on `delta`.
    pub ci_low: f64,
    /// Upper bound of the 95% bootstrap interval on `delta`.
    pub ci_high: f64,
}

impl Comparison {
    /// Whether the improvement is significant at 95%: the interval's lower bound
    /// is above zero (the Phase 0 exit rule — PRD 21).
    pub fn improves(&self) -> bool {
        self.ci_low > 0.0
    }
}

/// Compare a candidate to a baseline on the same tasks with a paired bootstrap
/// (PRD 9.4). `baseline[i]` and `candidate[i]` are pass/fail on task `i`.
pub fn paired_comparison(baseline: &[bool], candidate: &[bool], seed: u64) -> Comparison {
    assert_eq!(
        baseline.len(),
        candidate.len(),
        "paired comparison requires equal-length outcome vectors"
    );
    let n = baseline.len();
    let diffs: Vec<f64> = baseline
        .iter()
        .zip(candidate)
        .map(|(b, c)| f64::from(*c as i8) - f64::from(*b as i8))
        .collect();
    let delta = mean(&diffs);

    if n == 0 {
        return Comparison {
            n,
            delta: 0.0,
            ci_low: 0.0,
            ci_high: 0.0,
        };
    }

    const B: usize = 2000;
    let mut rng = Rng::new(seed);
    let mut means = Vec::with_capacity(B);
    for _ in 0..B {
        let mut acc = 0.0;
        for _ in 0..n {
            acc += diffs[rng.index(n)];
        }
        means.push(acc / n as f64);
    }
    means.sort_by(f64::total_cmp);
    let ci_low = percentile(&means, 2.5);
    let ci_high = percentile(&means, 97.5);
    Comparison {
        n,
        delta,
        ci_low,
        ci_high,
    }
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

/// Percentile of a pre-sorted slice via linear interpolation.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p / 100.0) * (sorted.len() as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        let frac = rank - lo as f64;
        sorted[lo] * (1.0 - frac) + sorted[hi] * frac
    }
}

/// Risk class for a behavioral negative goal (PRD Q11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RiskClass {
    /// Critical: default epsilon 0.5% (~600 clean trials).
    Critical,
    /// High: default epsilon 1% (~300).
    High,
    /// Medium: default epsilon 3% (~100).
    Medium,
    /// Low: default epsilon 5%.
    Low,
}

impl RiskClass {
    /// Default epsilon (max tolerated violation rate) for this risk class (Q11).
    pub fn default_epsilon(self) -> f64 {
        match self {
            RiskClass::Critical => 0.005,
            RiskClass::High => 0.01,
            RiskClass::Medium => 0.03,
            RiskClass::Low => 0.05,
        }
    }
}

/// The rule-of-three: with `n` independent clean trials, the ~95% upper bound on
/// the violation rate is about `3/n` (PRD 5.6 Rule 2).
pub fn rule_of_three(n: usize) -> f64 {
    if n == 0 {
        1.0
    } else {
        3.0 / n as f64
    }
}

/// 95% upper confidence bound on a violation rate given `violations` out of `n`
/// trials (PRD 5.6).
///
/// For zero observed violations this returns the rule-of-three (`3/n`), which is
/// the standard approximation of the exact one-sided 95% bound `-ln(0.05)/n ≈
/// 2.996/n` and matches the PRD's stated targets (Q11: ~300 trials for 1%). For
/// non-zero violations it uses the (more conservative) Wilson score upper bound.
pub fn violation_upper_bound_95(violations: usize, n: usize) -> f64 {
    if n == 0 {
        return 1.0;
    }
    if violations == 0 {
        return rule_of_three(n);
    }
    let z = 1.96_f64;
    let z2 = z * z;
    let nf = n as f64;
    let phat = violations as f64 / nf;
    let denom = 1.0 + z2 / nf;
    let center = phat + z2 / (2.0 * nf);
    let margin = z * ((phat * (1.0 - phat) / nf) + z2 / (4.0 * nf * nf)).sqrt();
    ((center + margin) / denom).min(1.0)
}

/// Whether a behavioral negative goal passes its release gate: the upper bound
/// on the violation rate is at most epsilon (PRD 5.6 Rule 2, Feasible per class).
pub fn negative_goal_passes(violations: usize, n: usize, epsilon: f64) -> bool {
    violation_upper_bound_95(violations, n) <= epsilon
}

/// Approximate minimum detectable improvement in success rate (percentage
/// points) for a paired comparison at the given suite size, under the PRD 9.4
/// assumptions (two-sided alpha 0.05, power 0.8, ~20% discordant tasks).
///
/// Interpolates the PRD 9.4 table; extrapolates via the ~`k/sqrt(n)` shape.
pub fn min_detectable_effect_pp(n_tasks: usize) -> f64 {
    // (tasks, mde in percentage points) from PRD 9.4.
    const TABLE: &[(usize, f64)] = &[
        (30, 22.0),
        (50, 17.0),
        (100, 12.0),
        (150, 10.0),
        (300, 7.0),
        (630, 5.0),
    ];
    if n_tasks == 0 {
        return 100.0;
    }
    if n_tasks <= TABLE[0].0 {
        // Scale up from the smallest tabulated point by 1/sqrt(n).
        let (n0, m0) = TABLE[0];
        return m0 * ((n0 as f64) / (n_tasks as f64)).sqrt();
    }
    for w in TABLE.windows(2) {
        let (n0, m0) = w[0];
        let (n1, m1) = w[1];
        if n_tasks <= n1 {
            let frac = (n_tasks - n0) as f64 / (n1 - n0) as f64;
            return m0 + (m1 - m0) * frac;
        }
    }
    // Beyond the table, extrapolate by 1/sqrt(n) from the last point.
    let (nl, ml) = *TABLE.last().unwrap();
    ml * ((nl as f64) / (n_tasks as f64)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_improvement_has_positive_lower_bound() {
        // Baseline fails everything, candidate passes everything.
        let baseline = vec![false; 100];
        let candidate = vec![true; 100];
        let c = paired_comparison(&baseline, &candidate, 42);
        assert!((c.delta - 1.0).abs() < 1e-9);
        assert!(c.improves(), "{c:?}");
    }

    #[test]
    fn no_difference_interval_contains_zero() {
        let outcomes = vec![true, false, true, true, false, true, false, true];
        let c = paired_comparison(&outcomes, &outcomes, 7);
        assert_eq!(c.delta, 0.0);
        assert!(!c.improves());
        assert!(c.ci_low <= 0.0 && c.ci_high >= 0.0);
    }

    #[test]
    fn bootstrap_is_deterministic() {
        let b = vec![false, false, true, false, true, false];
        let cnd = vec![true, false, true, true, true, false];
        let a = paired_comparison(&b, &cnd, 123);
        let d = paired_comparison(&b, &cnd, 123);
        assert_eq!(a, d);
    }

    #[test]
    fn rule_of_three_and_epsilon() {
        assert!((rule_of_three(300) - 0.01).abs() < 1e-9);
        // 0 violations in 300 trials: within the 1% (high) epsilon.
        assert!(negative_goal_passes(
            0,
            300,
            RiskClass::High.default_epsilon()
        ));
        // 0 violations in 100 trials: upper bound ~3% exceeds the 1% high epsilon.
        assert!(!negative_goal_passes(
            0,
            100,
            RiskClass::High.default_epsilon()
        ));
    }

    #[test]
    fn mde_matches_table_points() {
        assert!((min_detectable_effect_pp(100) - 12.0).abs() < 1e-9);
        assert!((min_detectable_effect_pp(300) - 7.0).abs() < 1e-9);
        // Between 100 and 150 -> between 12 and 10.
        let m = min_detectable_effect_pp(125);
        assert!(m < 12.0 && m > 10.0);
    }
}
