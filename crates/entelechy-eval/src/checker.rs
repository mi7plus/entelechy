//! Checkers (PRD EV-4). Phase 0 delivers programmatic checkers plus one rubric
//! judge; trajectory, differential and policy checkers arrive later.
//!
//! The Phase 0 primary metric must be decided by programmatic state checkers
//! wherever the simulator permits it; rubric-judge results are secondary unless
//! the StudyPlan pre-registers a human-calibrated judge (PRD 21.1.1).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::task::Task;

/// The kind of checker that produced an outcome (PRD EV-4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckerKind {
    /// Deterministic programmatic state check (preferred for the primary metric).
    Programmatic,
    /// Rubric judge (secondary; must be calibrated against humans — EV-7).
    RubricJudge,
    /// Policy checker (a negative-goal / authority check).
    Policy,
}

/// The outcome of running a checker on one task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckOutcome {
    /// Whether the task passed. For a negative-goal checker, `true` means the
    /// forbidden outcome did NOT occur.
    pub passed: bool,
    /// Optional continuous score in `[0,1]` (rubric judges).
    pub score: Option<f64>,
    /// The kind of checker.
    pub kind: CheckerKind,
    /// Whether this represents a negative-goal (forbidden-outcome) check.
    pub negative_goal: bool,
    /// Human-readable detail.
    pub detail: String,
}

impl CheckOutcome {
    /// A passing programmatic outcome.
    pub fn pass() -> Self {
        Self {
            passed: true,
            score: None,
            kind: CheckerKind::Programmatic,
            negative_goal: false,
            detail: String::new(),
        }
    }

    /// A failing programmatic outcome with detail.
    pub fn fail(detail: impl Into<String>) -> Self {
        Self {
            passed: false,
            score: None,
            kind: CheckerKind::Programmatic,
            negative_goal: false,
            detail: detail.into(),
        }
    }
}

/// A checker decides a task given the system-under-test output (PRD EV-4).
pub trait Checker {
    /// Evaluate one task's output.
    fn check(&self, task: &Task, output: &serde_json::Value) -> CheckOutcome;
}

/// A programmatic checker backed by a closure.
pub struct Programmatic<F>(pub F)
where
    F: Fn(&Task, &serde_json::Value) -> CheckOutcome;

impl<F> Checker for Programmatic<F>
where
    F: Fn(&Task, &serde_json::Value) -> CheckOutcome,
{
    fn check(&self, task: &Task, output: &serde_json::Value) -> CheckOutcome {
        (self.0)(task, output)
    }
}

/// A registry of named checkers (EV-4).
#[derive(Default)]
pub struct CheckerRegistry {
    checkers: HashMap<String, Box<dyn Checker + Send + Sync>>,
}

impl CheckerRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a checker under a name.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        checker: impl Checker + Send + Sync + 'static,
    ) {
        self.checkers.insert(name.into(), Box::new(checker));
    }

    /// Register a programmatic checker from a closure.
    pub fn register_fn(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&Task, &serde_json::Value) -> CheckOutcome + Send + Sync + 'static,
    ) {
        self.register(name, Programmatic(f));
    }

    /// Run all of a task's checkers over an output. The task passes only if every
    /// checker passes (a negative-goal violation fails the task).
    pub fn run_task(&self, task: &Task, output: &serde_json::Value) -> Vec<CheckOutcome> {
        task.checkers
            .iter()
            .map(|name| match self.checkers.get(name) {
                Some(c) => c.check(task, output),
                None => CheckOutcome::fail(format!("unknown checker '{name}'")),
            })
            .collect()
    }

    /// Whether a task passes all its checkers.
    pub fn task_passes(&self, task: &Task, output: &serde_json::Value) -> bool {
        self.run_task(task, output).iter().all(|o| o.passed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{Difficulty, Provenance, Split, Task};

    fn task(checkers: &[&str]) -> Task {
        Task {
            id: "t".into(),
            input: serde_json::json!({}),
            environment: "e".into(),
            checkers: checkers.iter().map(|s| s.to_string()).collect(),
            tags: vec![],
            difficulty: Difficulty::Easy,
            provenance: Provenance::HumanSeed,
            split: Split::Tune,
            source_task: None,
        }
    }

    #[test]
    fn programmatic_checker_runs() {
        let mut reg = CheckerRegistry::new();
        reg.register_fn("has_reply", |_t, out| {
            if out.get("reply").is_some() {
                CheckOutcome::pass()
            } else {
                CheckOutcome::fail("no reply")
            }
        });
        let t = task(&["has_reply"]);
        assert!(reg.task_passes(&t, &serde_json::json!({"reply": "hi"})));
        assert!(!reg.task_passes(&t, &serde_json::json!({})));
    }

    #[test]
    fn unknown_checker_fails_closed() {
        let reg = CheckerRegistry::new();
        let t = task(&["missing"]);
        assert!(!reg.task_passes(&t, &serde_json::json!({})));
    }
}
