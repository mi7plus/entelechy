//! BenchmarkManifest: task provenance, split assignment, lineage and
//! contamination status (PRD 5.2, 9.5). Immutable per release; holdout content
//! is reachable only through the gate API (EV-14).

use serde::{Deserialize, Serialize};

use entelechy_artifacts::ArtifactId;
use entelechy_eval::{Provenance, Split, Suite, Task};

use crate::contamination::{assess, Contamination};

/// One task's manifest row (content is referenced by hash, not embedded, so a
/// manifest can be published without leaking holdout content — 9.5, EV-14).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Task id.
    pub id: String,
    /// Split assignment.
    pub split: Split,
    /// Provenance for contamination control.
    pub provenance: Provenance,
    /// Source task id for synthetic lineage (9.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_task: Option<String>,
    /// Content address of the task's canonical bytes.
    pub content_hash: String,
}

/// A contamination finding between two tasks in different splits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrossSplitFinding {
    /// Task in the "protected" (validation/holdout) split.
    pub protected: String,
    /// Task in the source (tune/regression) split.
    pub other: String,
    /// The verdict.
    pub verdict: String,
}

/// An immutable benchmark manifest (PRD 5.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkManifest {
    /// Human-readable benchmark name.
    pub name: String,
    /// Manifest schema version.
    pub version: u32,
    /// One row per task.
    pub entries: Vec<ManifestEntry>,
}

impl BenchmarkManifest {
    /// Schema id for the artifact (PRD 5.9).
    pub const SCHEMA_ID: &'static str = "entelechy.benchmarkmanifest";

    /// Build a manifest from a suite, hashing each task's input as its content
    /// reference.
    pub fn from_suite(name: impl Into<String>, suite: &Suite) -> Self {
        let entries = suite
            .tasks
            .iter()
            .map(|t| ManifestEntry {
                id: t.id.clone(),
                split: t.split,
                provenance: t.provenance,
                source_task: t.source_task.clone(),
                content_hash: task_content_hash(t),
            })
            .collect();
        Self {
            name: name.into(),
            version: 1,
            entries,
        }
    }

    /// Content address of the whole manifest (its commitment).
    pub fn id(&self) -> ArtifactId {
        ArtifactId::of(self).expect("manifest is serializable")
    }

    /// The committed holdout content hashes, sealed before optimization (EL-2).
    pub fn holdout_hashes(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.split == Split::Holdout)
            .map(|e| e.content_hash.clone())
            .collect()
    }

    /// Scan for near-duplicate tasks that cross from tune/regression into
    /// validation/holdout (9.5, Q18). Returns blocking/flagged findings.
    ///
    /// `text_of` extracts comparable text from a task's manifest identity; here
    /// we compare the raw `Suite` tasks the manifest was built from.
    pub fn cross_split_scan(&self, suite: &Suite) -> Vec<CrossSplitFinding> {
        let text = |id: &str| -> Option<String> {
            suite
                .tasks
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.input.to_string())
        };
        let protected: Vec<&ManifestEntry> = self
            .entries
            .iter()
            .filter(|e| matches!(e.split, Split::Validation | Split::Holdout))
            .collect();
        let source: Vec<&ManifestEntry> = self
            .entries
            .iter()
            .filter(|e| matches!(e.split, Split::Tune | Split::Regression))
            .collect();

        let mut findings = Vec::new();
        for p in &protected {
            let Some(pt) = text(&p.id) else { continue };
            for s in &source {
                let Some(st) = text(&s.id) else { continue };
                let verdict = assess(&pt, &st, None);
                if verdict != Contamination::Clear {
                    findings.push(CrossSplitFinding {
                        protected: p.id.clone(),
                        other: s.id.clone(),
                        verdict: format!("{verdict:?}"),
                    });
                }
            }
        }
        findings
    }
}

fn task_content_hash(task: &Task) -> String {
    ArtifactId::of(&task.input)
        .map(|id| id.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_eval::{Difficulty, Provenance, Split, Suite, Task};

    fn t(id: &str, split: Split, input: serde_json::Value) -> Task {
        Task {
            id: id.into(),
            input,
            environment: "helpdesk".into(),
            checkers: vec!["state".into()],
            tags: vec![],
            difficulty: Difficulty::Medium,
            provenance: Provenance::HumanSeed,
            split,
            source_task: None,
        }
    }

    #[test]
    fn manifest_seals_holdout_hashes() {
        let suite = Suite::new(vec![
            t("a", Split::Tune, serde_json::json!({"q": "reset password"})),
            t("h", Split::Holdout, serde_json::json!({"q": "refund order"})),
        ]);
        let m = BenchmarkManifest::from_suite("helpdesk", &suite);
        assert_eq!(m.holdout_hashes().len(), 1);
        // Manifest id is stable.
        assert_eq!(m.id(), m.id());
    }

    #[test]
    fn cross_split_duplicate_is_detected() {
        let dup = serde_json::json!({"q": "please refund my broken item today"});
        let suite = Suite::new(vec![
            t("tune1", Split::Tune, dup.clone()),
            t("hold1", Split::Holdout, dup),
        ]);
        let m = BenchmarkManifest::from_suite("helpdesk", &suite);
        let findings = m.cross_split_scan(&suite);
        assert!(!findings.is_empty());
        assert_eq!(findings[0].verdict, "Blocked");
    }
}
