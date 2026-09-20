//! Evaluation: tasks, checkers, contract, statistics and split discipline.
//!
//! PRD v11 references: section 9 (evaluation compiler & engine), 5.6 (constraint
//! classes), 9.4 (statistical validity & power), 9.6 (leakage firewall).
//!
//! Evaluation is a governed product subsystem, not a utility library (PRD 9):
//! Entelechy compiles an [`EvalContract`] from the GoalSpec, then generates
//! concrete [`Suite`]s under that contract. The holdout is reachable only through
//! the audited, budgeted [`HoldoutVault`] gate (EV-14).
#![forbid(unsafe_code)]

pub mod adversarial;
pub mod checker;
pub mod contract;
pub mod holdout;
pub mod judge;
pub mod retrieval;
pub mod stats;
pub mod task;

pub use adversarial::{generate_variant, generate_variants, AdversarialKind};
pub use checker::{CheckOutcome, Checker, CheckerKind, CheckerRegistry, Programmatic};
pub use contract::{
    ConstraintClass, EvalContract, NegativeGoal, PowerWarning, ReleaseRule, SplitPolicy,
};
pub use holdout::{
    AuditEntry, CallerIdentity, GateResponse, HoldoutVault, InformationClass, Plane, VaultError,
};
pub use judge::{
    agreement_rate, cohens_kappa, ensemble, precision_recall, self_preference_bias, EnsembleVerdict,
};
pub use retrieval::{
    mean_reciprocal_rank, ndcg_at_k, precision_at_k, recall_at_k, reciprocal_rank,
};
pub use stats::{
    min_detectable_effect_pp, negative_goal_passes, paired_comparison, rule_of_three,
    violation_upper_bound_95, Comparison, RiskClass,
};
pub use task::{Difficulty, Provenance, Split, Suite, Task};
