//! Task synthesis with governance (PRD EV-3, Q2, 9.5, EV-17).
//!
//! Tasks are generated from a spec/examples via a model (EV-3), but generation is
//! gated and governed:
//! - A minimum human seed set is required before synthetic expansion (Q2): ≥30
//!   seeds, ≥3 per success criterion and per negative goal, ≥5 adversarial.
//! - Generated candidates are checked for contamination and near-duplication
//!   before crossing splits (9.5); a candidate that duplicates a protected task is
//!   rejected.
//! - Synthetic tasks may enter tune and validation, never the holdout directly
//!   (Q2 — a holdout entry requires full human review).
//! - The generating model family should differ from the family under test
//!   (EV-17); that is a wiring choice the caller makes by passing an appropriate
//!   [`entelechy_gateway::ModelGateway`].

use entelechy_eval::{Difficulty, Provenance, Split, Suite, Task};
use entelechy_gateway::{ModelGateway, ModelRequest};

use crate::contamination::{assess, Contamination};

/// Q2 minimum human-authored seeds per objective.
pub const MIN_SEEDS: usize = 30;
/// Q2 minimum seeds per success criterion and per negative goal.
pub const MIN_PER_TARGET: usize = 3;
/// Q2 minimum adversarial seeds.
pub const MIN_ADVERSARIAL: usize = 5;

/// A shortfall in the human seed set (PRD Q2).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SeedDeficiency {
    /// Fewer than [`MIN_SEEDS`] human seed tasks.
    TooFewSeeds {
        /// How many were provided.
        have: usize,
    },
    /// A criterion or negative goal has fewer than [`MIN_PER_TARGET`] seeds.
    TargetUndercovered {
        /// The criterion / negative-goal tag.
        target: String,
        /// How many cover it.
        have: usize,
    },
    /// Fewer than [`MIN_ADVERSARIAL`] adversarial seeds.
    TooFewAdversarial {
        /// How many adversarial seeds were provided.
        have: usize,
    },
}

/// Check the human seed set is sufficient before synthetic expansion (Q2).
/// `targets` are the success-criterion and negative-goal tags that must each be
/// covered by at least [`MIN_PER_TARGET`] human seeds.
pub fn check_seed_set(seeds: &[Task], targets: &[&str]) -> Result<(), Vec<SeedDeficiency>> {
    let human: Vec<&Task> = seeds
        .iter()
        .filter(|t| t.provenance == Provenance::HumanSeed)
        .collect();
    let mut deficiencies = Vec::new();

    if human.len() < MIN_SEEDS {
        deficiencies.push(SeedDeficiency::TooFewSeeds { have: human.len() });
    }
    for target in targets {
        let have = human.iter().filter(|t| t.has_tag(target)).count();
        if have < MIN_PER_TARGET {
            deficiencies.push(SeedDeficiency::TargetUndercovered {
                target: (*target).to_string(),
                have,
            });
        }
    }
    let adversarial = human.iter().filter(|t| t.has_tag("adversarial")).count();
    if adversarial < MIN_ADVERSARIAL {
        deficiencies.push(SeedDeficiency::TooFewAdversarial { have: adversarial });
    }

    if deficiencies.is_empty() {
        Ok(())
    } else {
        Err(deficiencies)
    }
}

/// Errors from synthesis.
#[derive(Debug, thiserror::Error)]
pub enum SynthesisError {
    /// Synthetic tasks may not be placed directly into the holdout (Q2).
    #[error("synthetic tasks cannot enter the holdout directly (PRD Q2)")]
    HoldoutForbidden,
    /// The model output could not be parsed into tasks.
    #[error("could not parse generated tasks: {0}")]
    Parse(String),
    /// The model call failed.
    #[error("generation failed: {0}")]
    Generation(String),
}

/// Parse a model's JSON array of task inputs into [`Task`]s (EV-3). Each element
/// is either a task `input` object or `{ "input": ... }`. Produced tasks carry
/// `Synthetic` provenance and a lineage link to `source_task` (9.5).
pub fn parse_generated_tasks(
    json: &str,
    environment: &str,
    checkers: &[String],
    split: Split,
    source_task: &str,
    id_prefix: &str,
) -> Result<Vec<Task>, SynthesisError> {
    if split == Split::Holdout {
        return Err(SynthesisError::HoldoutForbidden);
    }
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| SynthesisError::Parse(e.to_string()))?;
    let arr = value
        .as_array()
        .ok_or_else(|| SynthesisError::Parse("expected a JSON array".into()))?;
    let mut out = Vec::new();
    for (i, el) in arr.iter().enumerate() {
        let input = el.get("input").cloned().unwrap_or_else(|| el.clone());
        out.push(Task {
            id: format!("{id_prefix}-{i}"),
            input,
            environment: environment.to_string(),
            checkers: checkers.to_vec(),
            tags: vec!["synthetic".to_string()],
            difficulty: Difficulty::Medium,
            provenance: Provenance::Synthetic,
            split,
            source_task: Some(source_task.to_string()),
        });
    }
    Ok(out)
}

/// Admit generated candidates into a suite (9.5): reject any that duplicate an
/// existing task (blocking contamination) and refuse holdout placement (Q2).
/// Returns the admitted tasks. `text_of` extracts comparable text from a task.
pub fn admit_candidates(
    candidates: Vec<Task>,
    existing: &Suite,
    target_split: Split,
) -> Result<Vec<Task>, SynthesisError> {
    if target_split == Split::Holdout {
        return Err(SynthesisError::HoldoutForbidden);
    }
    let existing_texts: Vec<String> = existing.tasks.iter().map(|t| t.input.to_string()).collect();
    let mut admitted = Vec::new();
    for mut cand in candidates {
        cand.split = target_split;
        let cand_text = cand.input.to_string();
        let blocked = existing_texts
            .iter()
            .any(|e| assess(&cand_text, e, None) == Contamination::Blocked);
        if !blocked {
            admitted.push(cand);
        }
    }
    Ok(admitted)
}

/// A model-backed task generator (EV-3). Prompts a [`ModelGateway`] for variations
/// of a seed task and parses the JSON result. The caller should pass a model from
/// a different family than the system under test (EV-17).
pub struct ModelTaskGenerator<'a> {
    /// The generating model (ideally a different family than under test — EV-17).
    pub model: &'a dyn ModelGateway,
    /// The model identity to request.
    pub model_id: String,
}

impl<'a> ModelTaskGenerator<'a> {
    /// Create a generator.
    pub fn new(model: &'a dyn ModelGateway, model_id: impl Into<String>) -> Self {
        Self {
            model,
            model_id: model_id.into(),
        }
    }

    /// Generate `n` synthetic variations of `seed` for `target_split` (EV-3). The
    /// prompt asks for a JSON array of task inputs; the response is parsed and
    /// tagged with `Synthetic` provenance and lineage. Holdout is refused (Q2).
    pub fn generate(
        &self,
        seed: &Task,
        n: usize,
        target_split: Split,
    ) -> Result<Vec<Task>, SynthesisError> {
        if target_split == Split::Holdout {
            return Err(SynthesisError::HoldoutForbidden);
        }
        let prompt = format!(
            "Generate {n} diverse variations of this task as a JSON array of objects, \
             each with an \"input\" field. Base task input: {}",
            seed.input
        );
        let resp = self
            .model
            .infer(&ModelRequest {
                model: self.model_id.clone(),
                prompt,
                temperature: 0.7,
            })
            .map_err(|e| SynthesisError::Generation(e.to_string()))?;
        parse_generated_tasks(
            &resp.text,
            &seed.environment,
            &seed.checkers,
            target_split,
            &seed.id,
            &format!("{}~syn", seed.id),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_eval::{Difficulty, Provenance, Split, Suite, Task};

    fn seed(id: &str, tags: &[&str]) -> Task {
        Task {
            id: id.into(),
            input: serde_json::json!({ "q": id }),
            environment: "helpdesk".into(),
            checkers: vec!["state".into()],
            tags: tags.iter().map(|s| s.to_string()).collect(),
            difficulty: Difficulty::Medium,
            provenance: Provenance::HumanSeed,
            split: Split::Tune,
            source_task: None,
        }
    }

    #[test]
    fn seed_set_gate_enforces_q2_minimums() {
        // Too few seeds.
        let few = vec![seed("a", &["resolves"])];
        let errs = check_seed_set(&few, &["resolves"]).unwrap_err();
        assert!(errs
            .iter()
            .any(|d| matches!(d, SeedDeficiency::TooFewSeeds { .. })));

        // 30 seeds, 3 per target, 5 adversarial -> OK.
        let mut seeds: Vec<Task> = (0..30)
            .map(|i| seed(&format!("s{i}"), &["resolves"]))
            .collect();
        for t in seeds.iter_mut().take(5) {
            t.tags.push("adversarial".into());
        }
        assert!(check_seed_set(&seeds, &["resolves"]).is_ok());
    }

    #[test]
    fn holdout_placement_is_refused() {
        assert!(matches!(
            parse_generated_tasks("[]", "e", &[], Split::Holdout, "src", "p"),
            Err(SynthesisError::HoldoutForbidden)
        ));
        let admit = admit_candidates(vec![], &Suite::default(), Split::Holdout);
        assert!(matches!(admit, Err(SynthesisError::HoldoutForbidden)));
    }

    #[test]
    fn parses_generated_tasks_with_lineage() {
        let json = r#"[{"input":{"q":"reset password"}},{"input":{"q":"refund status"}}]"#;
        let tasks = parse_generated_tasks(
            json,
            "helpdesk",
            &["state".into()],
            Split::Tune,
            "seed-1",
            "seed-1~syn",
        )
        .unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].provenance, Provenance::Synthetic);
        assert_eq!(tasks[0].source_task.as_deref(), Some("seed-1"));
        assert!(tasks[0].has_tag("synthetic"));
    }

    #[test]
    fn admit_rejects_near_duplicates_of_existing() {
        let existing = Suite::new(vec![seed("hold", &[])]); // input {"q":"hold"}
                                                            // A candidate identical to an existing task is blocked (9.5).
        let dup = Task {
            id: "c0".into(),
            ..seed("hold", &[])
        };
        let admitted = admit_candidates(vec![dup], &existing, Split::Tune).unwrap();
        assert!(admitted.is_empty());
        // A distinct candidate is admitted.
        let distinct = Task {
            id: "c1".into(),
            input: serde_json::json!({"q":"totally different request text"}),
            ..seed("x", &[])
        };
        let admitted = admit_candidates(vec![distinct], &existing, Split::Tune).unwrap();
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].split, Split::Tune);
    }

    #[test]
    fn model_generator_parses_a_canned_response() {
        // A stub gateway returning a fixed JSON array.
        struct StubModel;
        impl ModelGateway for StubModel {
            fn infer(
                &self,
                _req: &ModelRequest,
            ) -> Result<entelechy_gateway::ModelResponse, entelechy_gateway::ModelError>
            {
                Ok(entelechy_gateway::ModelResponse {
                    text: r#"[{"input":{"q":"v1"}},{"input":{"q":"v2"}}]"#.into(),
                    identity: entelechy_gateway::ProviderIdentity {
                        provider: "stub".into(),
                        advertised_model: "stub".into(),
                        endpoint: "test".into(),
                        request_schema_version: "1".into(),
                        sampling: serde_json::json!({}),
                        revision: None,
                    },
                })
            }
        }
        let model = StubModel;
        let gen = ModelTaskGenerator::new(&model, "generator-model");
        let tasks = gen
            .generate(&seed("seed-1", &[]), 2, Split::Validation)
            .unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].split, Split::Validation);
        assert_eq!(tasks[1].source_task.as_deref(), Some("seed-1"));
    }
}
