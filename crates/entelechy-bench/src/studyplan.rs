//! StudyPlan: signed pre-registration of a study (PRD 5.2, 21.1).
//!
//! Before the first optimization run, the repository contains a signed StudyPlan
//! that freezes the benchmark manifest, baseline, primary metric, negative goals,
//! split sizes, minimum detectable effect, candidate/search budget, stopping rule,
//! analysis method and frozen model identities (PRD 21.1). Changes after results
//! are observed start a new study; extending the budget creates a new revision and
//! invalidates claims that depended on the original stopping rule.

use serde::{Deserialize, Serialize};

use entelechy_artifacts::ArtifactId;
use entelechy_eval::SplitPolicy;

/// The stopping rule for a study (PRD 11.8, 21.1, Q23).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StoppingRule {
    /// Phase 0 default: hard budget cap plus a single non-binding futility check
    /// at half budget; no early stopping for success (PRD 21.1).
    Phase0Default {
        /// Maximum candidates the study may evaluate (candidate accounting —
        /// includes failed/invalid/abandoned candidates, PRD 21.1).
        candidate_budget: u32,
    },
    /// Phase 2+ group-sequential design with an O'Brien–Fleming alpha-spending
    /// function and at most four pre-registered looks (Q23).
    GroupSequential {
        /// Candidate budget.
        candidate_budget: u32,
        /// Look points as fractions of the budget (≤ 4 looks, Q23).
        looks: Vec<f64>,
    },
}

impl StoppingRule {
    /// The candidate budget regardless of variant.
    pub fn candidate_budget(&self) -> u32 {
        match self {
            StoppingRule::Phase0Default { candidate_budget }
            | StoppingRule::GroupSequential {
                candidate_budget, ..
            } => *candidate_budget,
        }
    }
}

/// A signed pre-registration of a study (PRD 21.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StudyPlan {
    /// Schema/plan version (a revision after results invalidates prior claims).
    pub revision: u32,
    /// Content address of the frozen [`crate::BenchmarkManifest`].
    pub manifest_id: String,
    /// Content address of the compiler-valid baseline design (PRD 21.1).
    pub baseline_id: String,
    /// Primary metric name (programmatic where possible — 21.1.1).
    pub primary_metric: String,
    /// Split sizes.
    pub splits: SplitPolicy,
    /// Minimum detectable effect (pp) for each confirmatory split (EV-15).
    pub mde_validation_pp: f64,
    /// Minimum detectable effect (pp) for the holdout split (EV-15).
    pub mde_holdout_pp: f64,
    /// Stopping rule and candidate accounting.
    pub stopping: StoppingRule,
    /// Analysis method (e.g. "paired bootstrap, 95% interval").
    pub analysis: String,
    /// Frozen model identities allowed in the study (PRD 21.1, 5.10).
    pub frozen_models: Vec<String>,
    /// The principal that signed the pre-registration.
    pub signed_by: String,
}

impl StudyPlan {
    /// Schema id for the artifact (PRD 5.9).
    pub const SCHEMA_ID: &'static str = "entelechy.studyplan";

    /// Content address of this plan: its signature commitment. A material change
    /// yields a different id, which is how "a change after results starts a new
    /// study" is detected (PRD 21.1).
    pub fn id(&self) -> ArtifactId {
        ArtifactId::of(self).expect("study plan is serializable")
    }

    /// Whether `other` is a material change from `self` (different commitment).
    /// Any field change is material for a pre-registered plan (PRD 21.1).
    pub fn is_material_change(&self, other: &StudyPlan) -> bool {
        self.id() != other.id()
    }

    /// The candidate budget for the study.
    pub fn candidate_budget(&self) -> u32 {
        self.stopping.candidate_budget()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> StudyPlan {
        StudyPlan {
            revision: 1,
            manifest_id: "sha256:aa".into(),
            baseline_id: "sha256:bb".into(),
            primary_metric: "task_success".into(),
            splits: SplitPolicy::default(),
            mde_validation_pp: 17.0,
            mde_holdout_pp: 12.0,
            stopping: StoppingRule::Phase0Default {
                candidate_budget: 40,
            },
            analysis: "paired bootstrap, 95% interval".into(),
            frozen_models: vec!["mock-small@r1".into()],
            signed_by: "owner@example".into(),
        }
    }

    #[test]
    fn plan_id_is_stable() {
        assert_eq!(plan().id(), plan().id());
        assert_eq!(plan().candidate_budget(), 40);
    }

    #[test]
    fn budget_change_is_material() {
        let a = plan();
        let mut b = plan();
        b.stopping = StoppingRule::Phase0Default {
            candidate_budget: 80,
        };
        assert!(a.is_material_change(&b));
    }
}
