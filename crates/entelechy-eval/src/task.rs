//! Task model and split discipline (PRD 9.3 EV-1, EV-8, 9.5).

use serde::{Deserialize, Serialize};

/// Evaluation split (PRD EV-8). An information gradient runs tune -> validation
/// -> holdout (PRD 9.6): tune may return task-level diagnostics; holdout is
/// reachable only through the gate API (EV-14).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Split {
    /// Development / smoke split.
    Dev,
    /// Tune split: candidates are selected here.
    Tune,
    /// Rolling-validation split: accepted mutations are confirmed here (EV-13).
    Validation,
    /// Holdout split: consulted only by the release-gate protocol (EV-14).
    Holdout,
    /// Regression split: closed failures and published holdouts live here (EV-12).
    Regression,
}

/// Where a task came from (PRD 9.5 benchmark governance).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provenance {
    /// Human-authored seed task (PRD Q2 requires >= 30 per objective).
    HumanSeed,
    /// Synthetically generated, with lineage to a source task.
    Synthetic,
    /// Derived from a production incident (EV-12). Never independent holdout.
    Incident,
    /// A public benchmark task (reported separately; external validity only).
    Public,
}

/// Difficulty tag for a task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Difficulty {
    /// Straightforward.
    Easy,
    /// Typical.
    Medium,
    /// Hard / edge case.
    Hard,
}

/// A single evaluation task (PRD EV-1): inputs, environment, checkers, tags,
/// difficulty and provenance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    /// Stable task identifier.
    pub id: String,
    /// Input payload handed to the system under test.
    pub input: serde_json::Value,
    /// Named environment/simulator this task runs against (resettable — EV-5).
    pub environment: String,
    /// Names of the checkers that decide this task (EV-4).
    pub checkers: Vec<String>,
    /// Free-form tags (e.g. success criterion or negative goal it exercises).
    pub tags: Vec<String>,
    /// Difficulty tag.
    pub difficulty: Difficulty,
    /// Provenance for contamination control (9.5).
    pub provenance: Provenance,
    /// The split this task belongs to.
    pub split: Split,
    /// For synthetic tasks, the source task id (split-lineage — 9.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_task: Option<String>,
}

impl Task {
    /// Whether this task carries the given tag.
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }
}

/// A named collection of tasks partitioned by split.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Suite {
    /// All tasks.
    pub tasks: Vec<Task>,
}

impl Suite {
    /// Build a suite from tasks.
    pub fn new(tasks: Vec<Task>) -> Self {
        Self { tasks }
    }

    /// Tasks in a given split.
    pub fn split(&self, split: Split) -> impl Iterator<Item = &Task> {
        self.tasks.iter().filter(move |t| t.split == split)
    }

    /// Count of tasks in a split.
    pub fn split_len(&self, split: Split) -> usize {
        self.split(split).count()
    }

    /// Number of human seed tasks (PRD Q2 minimum-seed check).
    pub fn human_seed_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|t| t.provenance == Provenance::HumanSeed)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, split: Split, prov: Provenance) -> Task {
        Task {
            id: id.into(),
            input: serde_json::json!({}),
            environment: "helpdesk".into(),
            checkers: vec!["state".into()],
            tags: vec![],
            difficulty: Difficulty::Medium,
            provenance: prov,
            split,
            source_task: None,
        }
    }

    #[test]
    fn split_counts() {
        let s = Suite::new(vec![
            task("a", Split::Tune, Provenance::HumanSeed),
            task("b", Split::Tune, Provenance::Synthetic),
            task("c", Split::Holdout, Provenance::HumanSeed),
        ]);
        assert_eq!(s.split_len(Split::Tune), 2);
        assert_eq!(s.split_len(Split::Holdout), 1);
        assert_eq!(s.human_seed_count(), 2);
    }
}
