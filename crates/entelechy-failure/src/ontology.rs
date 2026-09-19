//! Failure ontology (PRD 10.1) and the class → complexity-level map (PRD 11.4).
//!
//! The ontology is a versioned contract (PRD 22 "failure ontology versioning").

use serde::{Deserialize, Serialize};

/// The version of the failure ontology (PRD 22 lock-early decision).
pub const ONTOLOGY_VERSION: u32 = 1;

/// A failure family (PRD 10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    /// Specification: ambiguity, missing/conflicting criteria, wrong assumption.
    Specification,
    /// Reasoning: decomposition, planning, synthesis, verification.
    Reasoning,
    /// Knowledge: retrieval miss, stale source, grounding, context overflow.
    Knowledge,
    /// Tool: selection, arguments, execution, result interpretation.
    Tool,
    /// Coordination: delegation, duplicated work, information loss, sync.
    Coordination,
    /// Policy: unauthorized action, taint violation, missing approval.
    Policy,
    /// Model: capability limit, schema failure, instability.
    Model,
    /// Evaluation: bad checker, judge disagreement, flaky env, missing coverage.
    Evaluation,
}

/// A specific failure class within a family (PRD 10.1). Identified by its stable
/// dotted name, e.g. `knowledge.context_overflow`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FailureClass {
    /// The family.
    pub family: Family,
    /// The class name within the family.
    pub name: &'static str,
}

impl FailureClass {
    /// Stable dotted identifier, e.g. `reasoning.verification`.
    pub fn id(&self) -> String {
        format!("{:?}.{}", self.family, self.name).to_lowercase()
    }

    /// Whether this class denotes a specification or evaluation defect rather
    /// than a design defect (PRD 10.2 distinguishes system vs evaluation failure;
    /// specification defects are fixed by re-specifying, not by design search).
    pub fn is_meta_defect(&self) -> bool {
        matches!(self.family, Family::Specification | Family::Evaluation)
    }

    /// The hierarchical search levels this class makes eligible (PRD 11.4).
    /// The seven levels are a default prior, not a strict ladder: each class maps
    /// to the levels it makes eligible (e.g. context overflow unlocks map/reduce
    /// (4) directly; a retrieval miss unlocks level 2 without passing through
    /// verification). Meta-defects unlock no design level (empty).
    pub fn eligible_levels(&self) -> &'static [u8] {
        match (self.family, self.name) {
            (Family::Knowledge, "retrieval_miss") => &[2],
            (Family::Knowledge, "stale_source") => &[2],
            (Family::Knowledge, "grounding_failure") => &[2, 3],
            (Family::Knowledge, "context_overflow") => &[2, 4],
            (Family::Reasoning, "verification") => &[3],
            (Family::Reasoning, "synthesis") => &[1, 3],
            (Family::Reasoning, "decomposition") => &[4, 5],
            (Family::Reasoning, "planning") => &[4, 5],
            (Family::Tool, _) => &[1],
            (Family::Coordination, _) => &[5],
            (Family::Model, _) => &[1],
            (Family::Policy, _) => &[3], // add a Gate / narrow authority (structural)
            _ => &[],
        }
    }
}

/// Construct a class.
pub const fn class(family: Family, name: &'static str) -> FailureClass {
    FailureClass { family, name }
}

/// The full ontology (PRD 10.1).
pub const ONTOLOGY: &[FailureClass] = &[
    class(Family::Specification, "ambiguity"),
    class(Family::Specification, "missing_criterion"),
    class(Family::Specification, "conflicting_constraint"),
    class(Family::Specification, "wrong_assumption"),
    class(Family::Reasoning, "decomposition"),
    class(Family::Reasoning, "planning"),
    class(Family::Reasoning, "synthesis"),
    class(Family::Reasoning, "verification"),
    class(Family::Knowledge, "retrieval_miss"),
    class(Family::Knowledge, "stale_source"),
    class(Family::Knowledge, "grounding_failure"),
    class(Family::Knowledge, "context_overflow"),
    class(Family::Tool, "selection"),
    class(Family::Tool, "arguments"),
    class(Family::Tool, "execution"),
    class(Family::Tool, "result_interpretation"),
    class(Family::Coordination, "bad_delegation"),
    class(Family::Coordination, "duplicated_work"),
    class(Family::Coordination, "information_loss"),
    class(Family::Coordination, "synchronization"),
    class(Family::Policy, "unauthorized_action"),
    class(Family::Policy, "taint_violation"),
    class(Family::Policy, "missing_approval"),
    class(Family::Model, "capability_limit"),
    class(Family::Model, "schema_failure"),
    class(Family::Model, "instability"),
    class(Family::Evaluation, "bad_checker"),
    class(Family::Evaluation, "judge_disagreement"),
    class(Family::Evaluation, "flaky_environment"),
    class(Family::Evaluation, "missing_coverage"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_levels() {
        let c = class(Family::Knowledge, "context_overflow");
        assert_eq!(c.id(), "knowledge.context_overflow");
        assert_eq!(c.eligible_levels(), &[2, 4]);
        assert!(!c.is_meta_defect());
    }

    #[test]
    fn spec_and_eval_are_meta_defects_with_no_level() {
        let s = class(Family::Specification, "ambiguity");
        assert!(s.is_meta_defect());
        assert!(s.eligible_levels().is_empty());
    }
}
