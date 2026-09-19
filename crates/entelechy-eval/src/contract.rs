//! EvalContract: what must be measured and how release decisions are made
//! (PRD 5.2, EV-2, EV-15, 9.4).
//!
//! The EvalContract is human-approved before optimization (EV-2) and fixes the
//! split policy, hard constraints, epsilon/delta and the minimum detectable
//! effect for each confirmatory split (EV-15). It is a governed artifact: eval
//! changes are versioned (PRD principle 3).

use serde::{Deserialize, Serialize};

use crate::stats::{min_detectable_effect_pp, RiskClass};
use crate::task::Split;

/// Constraint class of a goal (PRD 5.6). Feasibility is evaluated per class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConstraintClass {
    /// Enforced by static IR analysis and runtime policy; proven for the release.
    Structural,
    /// Enforced by prompts/verification/output checks; a statistical claim.
    Behavioral,
    /// Enforced by budget caps in the gateways; a distributional claim.
    Operational,
}

/// A negative goal / forbidden outcome (PRD 5.3, 5.6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NegativeGoal {
    /// Stable name, e.g. `no_refund`.
    pub name: String,
    /// Its constraint class.
    pub class: ConstraintClass,
    /// Risk class (drives the default epsilon — Q11). Only meaningful for
    /// behavioral goals.
    pub risk: RiskClass,
    /// Explicit epsilon override; when `None`, the risk-class default applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epsilon: Option<f64>,
    /// Confidence complement (default 0.05 per GoalSpec, PRD Q11).
    pub delta: f64,
}

impl NegativeGoal {
    /// The effective epsilon for this goal (override, else risk-class default).
    pub fn effective_epsilon(&self) -> f64 {
        self.epsilon.unwrap_or_else(|| self.risk.default_epsilon())
    }
}

/// The split policy: sizes and which split confirms a claim (PRD 9.4, Q1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitPolicy {
    /// Tune split size (candidates selected here).
    pub tune: usize,
    /// Rolling-validation split size (mutations confirmed here — EV-13).
    pub validation: usize,
    /// Holdout split size (release gate — EV-14).
    pub holdout: usize,
}

impl Default for SplitPolicy {
    /// The PRD Q1 default: 100 tune, 50 validation, 100 holdout.
    fn default() -> Self {
        Self {
            tune: 100,
            validation: 50,
            holdout: 100,
        }
    }
}

impl SplitPolicy {
    /// Size of a given confirmatory split.
    pub fn size(&self, split: Split) -> usize {
        match split {
            Split::Tune => self.tune,
            Split::Validation => self.validation,
            Split::Holdout => self.holdout,
            _ => 0,
        }
    }
}

/// The release rule for the primary comparison (PRD 21 exit, EV-2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReleaseRule {
    /// Name of the primary metric (e.g. `task_success`).
    pub primary_metric: String,
    /// Required improvement over baseline, in percentage points, that the
    /// confirmatory splits must detect (the pre-registered threshold).
    pub target_improvement_pp: f64,
    /// Query budget against the holdout gate (EV-14).
    pub holdout_query_budget: u32,
}

/// A power warning for a confirmatory split (PRD EV-15, OC-6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PowerWarning {
    /// The under-powered split.
    pub split: Split,
    /// The split's minimum detectable effect (pp).
    pub mde_pp: f64,
    /// The target improvement the study wants to confirm (pp).
    pub target_pp: f64,
}

/// A governed evaluation contract (PRD 5.2, EV-2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvalContract {
    /// Contract version (eval changes are versioned — PRD principle 3).
    pub version: u32,
    /// Named success criteria (specification tests — EV-1/9.1).
    pub criteria: Vec<String>,
    /// Negative goals and their constraint classes.
    pub negative_goals: Vec<NegativeGoal>,
    /// The split policy.
    pub splits: SplitPolicy,
    /// The release rule.
    pub release: ReleaseRule,
}

impl EvalContract {
    /// Schema id for the EvalContract artifact (PRD 5.9).
    pub const SCHEMA_ID: &'static str = "entelechy.evalcontract";

    /// Check that each confirmatory split can detect the target improvement
    /// (EV-15). Returns a warning per under-powered split; empty means OK.
    pub fn power_warnings(&self) -> Vec<PowerWarning> {
        let target = self.release.target_improvement_pp;
        [Split::Validation, Split::Holdout]
            .into_iter()
            .filter_map(|split| {
                let n = self.splits.size(split);
                let mde = min_detectable_effect_pp(n);
                (mde > target).then_some(PowerWarning {
                    split,
                    mde_pp: mde,
                    target_pp: target,
                })
            })
            .collect()
    }

    /// The behavioral negative goals that must be measured with an upper-bound
    /// test (PRD 5.6). Structural goals are proven, not measured.
    pub fn behavioral_goals(&self) -> impl Iterator<Item = &NegativeGoal> {
        self.negative_goals
            .iter()
            .filter(|g| g.class == ConstraintClass::Behavioral)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract(target_pp: f64) -> EvalContract {
        EvalContract {
            version: 1,
            criteria: vec!["resolves_ticket".into()],
            negative_goals: vec![
                NegativeGoal {
                    name: "no_refund".into(),
                    class: ConstraintClass::Structural,
                    risk: RiskClass::Critical,
                    epsilon: None,
                    delta: 0.05,
                },
                NegativeGoal {
                    name: "no_cross_customer_disclosure".into(),
                    class: ConstraintClass::Behavioral,
                    risk: RiskClass::High,
                    epsilon: None,
                    delta: 0.05,
                },
            ],
            splits: SplitPolicy::default(),
            release: ReleaseRule {
                primary_metric: "task_success".into(),
                target_improvement_pp: target_pp,
                holdout_query_budget: 5,
            },
        }
    }

    #[test]
    fn under_powered_holdout_warns() {
        // Default holdout is 100 tasks -> ~12pp MDE. Targeting 5pp is under-powered.
        let c = contract(5.0);
        let warnings = c.power_warnings();
        assert!(warnings.iter().any(|w| w.split == Split::Holdout));
    }

    #[test]
    fn adequately_powered_has_no_warning() {
        // Targeting 22pp is detectable by a 100-task holdout.
        let c = contract(22.0);
        assert!(c.power_warnings().is_empty());
    }

    #[test]
    fn behavioral_goal_epsilon_defaults_by_risk() {
        let c = contract(20.0);
        let g = c.behavioral_goals().next().unwrap();
        assert!((g.effective_epsilon() - 0.01).abs() < 1e-9); // High -> 1%
    }
}
