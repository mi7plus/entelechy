//! Objective compiler: GoalSpec construction, clarification and sign-off.
//!
//! PRD v11 references: section 5.3 (GoalSpec contract), 5.5 (objective compiler
//! requirements OC-1..OC-8), 5.7 (selection policy), 5.6 (constraint classes).
//!
//! The Objective Compiler carries its own requirements because every downstream
//! artifact inherits its errors (PRD 5.5). Each GoalSpec field is marked as
//! stated, inferred or defaulted (OC-1); inferred assumptions are linked to a
//! falsifying evaluation task (OC-4); sign-off produces an immutable GoalSpec and
//! any later change creates a new lineage branch (OC-8).
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

use entelechy_eval::NegativeGoal;
use entelechy_ir::AuthorityEnvelope;

/// A hard constraint in a constrained-optimization selection policy (PRD 5.7),
/// e.g. "task success ≥ 0.8" or "p95 latency ≤ 2000ms".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectionConstraint {
    /// Metric name.
    pub metric: String,
    /// Comparison operator, one of `>=`, `<=`, `>`, `<`, `==`.
    pub op: String,
    /// Threshold value.
    pub threshold: f64,
}

/// Whether a field was stated by the user, inferred by the compiler, or defaulted
/// (PRD OC-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FieldProvenance {
    /// Explicitly stated by the user.
    Stated,
    /// Inferred by the compiler (must be falsifiable — OC-4).
    Inferred,
    /// A system default.
    Defaulted,
}

/// A value annotated with its provenance (PRD OC-1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provenanced<T> {
    /// The value.
    pub value: T,
    /// How it was determined.
    pub provenance: FieldProvenance,
}

impl<T> Provenanced<T> {
    /// A stated value.
    pub fn stated(value: T) -> Self {
        Self { value, provenance: FieldProvenance::Stated }
    }
    /// An inferred value.
    pub fn inferred(value: T) -> Self {
        Self { value, provenance: FieldProvenance::Inferred }
    }
    /// A defaulted value.
    pub fn defaulted(value: T) -> Self {
        Self { value, provenance: FieldProvenance::Defaulted }
    }
}

/// The budget envelope (PRD 5.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetEnvelope {
    /// Money cap in minor currency units.
    pub money_minor: u64,
    /// Token cap.
    pub tokens: u64,
    /// Wall-clock cap in seconds.
    pub wall_secs: u64,
    /// Step cap.
    pub steps: u64,
}

/// The owner's selection policy (PRD 5.7). Weighted sums are deliberately not a
/// variant: they hide trade-offs and can let a constraint be traded for a score.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionPolicy {
    /// Constrained optimization (the default form, PRD 5.7): optimize one
    /// objective subject to hard constraints.
    ConstrainedOptimization {
        /// The objective to optimize, e.g. `minimize:cost`.
        objective: String,
        /// The constraints that define feasibility.
        constraints: Vec<SelectionConstraint>,
    },
    /// Lexicographic ordering (e.g. quality first, then cost).
    Lexicographic {
        /// Metric names in priority order.
        order: Vec<String>,
    },
    /// Explicit human choice among the Pareto set when the policy leaves ties.
    HumanChoice,
}

/// An assumption in the ledger (PRD OC-4). Each inferred assumption is linked to
/// at least one evaluation task that would falsify it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Assumption {
    /// The assumption text.
    pub text: String,
    /// The id of an evaluation task that would falsify it (OC-4).
    pub falsifying_task: Option<String>,
}

/// A clarification question ranked by expected impact (PRD OC-3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClarificationQuestion {
    /// The question text.
    pub text: String,
    /// Expected impact on design or evaluation, higher is more important.
    pub expected_impact: f64,
}

/// The outcome of a bounded clarification interview (PRD OC-3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InterviewResult {
    /// Questions to ask, highest impact first, up to the question budget.
    pub to_ask: Vec<ClarificationQuestion>,
    /// Questions deferred past the budget, recorded as explicit assumptions.
    pub deferred_assumptions: Vec<Assumption>,
}

/// Run a bounded clarification interview (OC-3): rank questions by expected
/// impact, stop at the question budget, and record unanswered ambiguities as
/// explicit (unfalsified) assumptions to be linked later.
pub fn run_interview(mut questions: Vec<ClarificationQuestion>, budget: usize) -> InterviewResult {
    questions.sort_by(|a, b| b.expected_impact.partial_cmp(&a.expected_impact).unwrap());
    let deferred = questions.split_off(budget.min(questions.len()));
    InterviewResult {
        to_ask: questions,
        deferred_assumptions: deferred
            .into_iter()
            .map(|q| Assumption { text: q.text, falsifying_task: None })
            .collect(),
    }
}

/// A machine-checkable contract for desired behavior (PRD 5.2, 5.3 GoalSpec).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalSpec {
    /// Measurable success criteria (PRD 5.3).
    pub success_criteria: Vec<Provenanced<String>>,
    /// Negative goals / forbidden outcomes, each classified (OC-2, reuses the
    /// eval crate's constraint-class model).
    pub negative_goals: Vec<NegativeGoal>,
    /// The maximum authority available to the system (PRD 5.4).
    pub authority: AuthorityEnvelope,
    /// Budget envelope.
    pub budget: Provenanced<BudgetEnvelope>,
    /// Selection policy chosen by the owner (PRD 5.7).
    pub selection_policy: SelectionPolicy,
    /// Assumption ledger (OC-4).
    pub assumptions: Vec<Assumption>,
    /// Data classification of inputs/outputs/stored data (PRD 5.3, 7.5).
    pub data_classification: Provenanced<String>,
    /// Whether the system acts on behalf of the requesting user (PRD 5.3, 5.8).
    pub on_behalf_of_user: bool,
    /// Value estimate in minor currency units, used to size study budgets.
    pub value_estimate_minor: Provenanced<u64>,
}

impl GoalSpec {
    /// Schema id for the artifact (PRD 5.9).
    pub const SCHEMA_ID: &'static str = "entelechy.goalspec";

    /// Whether the assumption ledger is valid (OC-4): every inferred assumption
    /// is linked to a falsifying evaluation task.
    pub fn assumption_ledger_valid(&self) -> bool {
        self.assumptions.iter().all(|a| a.falsifying_task.is_some())
    }

    /// Whether every negative goal is classified (OC-2). Classification is
    /// intrinsic to `NegativeGoal`, so this reports the count for the sign-off
    /// packet.
    pub fn negative_goal_count(&self) -> usize {
        self.negative_goals.len()
    }

    /// Sign off the GoalSpec, producing an immutable, content-addressed record
    /// (OC-8). Refuses if the assumption ledger is incomplete (OC-4).
    pub fn sign_off(self) -> Result<SignedGoalSpec, SignOffError> {
        if !self.assumption_ledger_valid() {
            return Err(SignOffError::UnfalsifiableAssumption);
        }
        let id = entelechy_artifacts::ArtifactId::of(&self)
            .map(|i| i.to_string())
            .map_err(|_| SignOffError::Serialization)?;
        Ok(SignedGoalSpec { goalspec: self, id })
    }
}

/// Errors from sign-off (PRD OC-4, OC-8).
#[derive(Debug, PartialEq, Eq)]
pub enum SignOffError {
    /// An inferred assumption has no falsifying task (OC-4).
    UnfalsifiableAssumption,
    /// The GoalSpec could not be serialized.
    Serialization,
}

impl std::fmt::Display for SignOffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignOffError::UnfalsifiableAssumption => {
                write!(f, "an inferred assumption is not linked to a falsifying task (OC-4)")
            }
            SignOffError::Serialization => write!(f, "GoalSpec serialization failed"),
        }
    }
}

impl std::error::Error for SignOffError {}

/// An immutable, signed GoalSpec (PRD OC-8). Any later change creates a new
/// lineage branch, detectable because the content hash differs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedGoalSpec {
    /// The GoalSpec.
    pub goalspec: GoalSpec,
    /// Its content address (the sign-off commitment).
    pub id: String,
}

impl SignedGoalSpec {
    /// Whether another GoalSpec is a material change from this one (OC-8): a
    /// different content hash starts a new lineage branch.
    pub fn is_new_branch(&self, other: &GoalSpec) -> bool {
        entelechy_artifacts::ArtifactId::of(other)
            .map(|i| i.to_string())
            .map(|id| id != self.id)
            .unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_eval::{ConstraintClass, NegativeGoal, RiskClass};

    fn base_goalspec() -> GoalSpec {
        GoalSpec {
            success_criteria: vec![Provenanced::stated("resolve tier-1 tickets".into())],
            negative_goals: vec![NegativeGoal {
                name: "no_refund".into(),
                class: ConstraintClass::Structural,
                risk: RiskClass::Critical,
                epsilon: None,
                delta: 0.05,
            }],
            authority: AuthorityEnvelope::empty(),
            budget: Provenanced::defaulted(BudgetEnvelope {
                money_minor: 60_000,
                tokens: 1_000_000,
                wall_secs: 3_600,
                steps: 100,
            }),
            selection_policy: SelectionPolicy::ConstrainedOptimization {
                objective: "minimize:cost".into(),
                constraints: vec![],
            },
            assumptions: vec![],
            data_classification: Provenanced::stated("internal".into()),
            on_behalf_of_user: false,
            value_estimate_minor: Provenanced::inferred(500_000),
        }
    }

    #[test]
    fn interview_ranks_and_defers() {
        let qs = vec![
            ClarificationQuestion { text: "q-low".into(), expected_impact: 0.1 },
            ClarificationQuestion { text: "q-high".into(), expected_impact: 0.9 },
            ClarificationQuestion { text: "q-mid".into(), expected_impact: 0.5 },
        ];
        let r = run_interview(qs, 1);
        assert_eq!(r.to_ask.len(), 1);
        assert_eq!(r.to_ask[0].text, "q-high");
        assert_eq!(r.deferred_assumptions.len(), 2);
        // Deferred questions become (as-yet unfalsified) assumptions.
        assert!(r.deferred_assumptions.iter().all(|a| a.falsifying_task.is_none()));
    }

    #[test]
    fn sign_off_requires_falsifiable_assumptions() {
        let mut gs = base_goalspec();
        gs.assumptions.push(Assumption { text: "users write English".into(), falsifying_task: None });
        assert_eq!(gs.clone().sign_off().unwrap_err(), SignOffError::UnfalsifiableAssumption);
        // Link a falsifying task and it signs off.
        gs.assumptions[0].falsifying_task = Some("task-lang-1".into());
        assert!(gs.sign_off().is_ok());
    }

    #[test]
    fn material_change_starts_new_branch() {
        let signed = base_goalspec().sign_off().unwrap();
        let mut changed = signed.goalspec.clone();
        assert!(!signed.is_new_branch(&changed)); // identical → same branch
        changed.on_behalf_of_user = true;
        assert!(signed.is_new_branch(&changed)); // material change → new branch
    }
}
