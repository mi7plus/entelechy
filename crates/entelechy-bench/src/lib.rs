//! Benchmark governance: manifests, split lineage, contamination and the signed
//! StudyPlan pre-registration.
//!
//! PRD v11 references: section 9.5 (benchmark governance & contamination
//! control), 21.1 (Phase 0 experiment protocol), 21.1.1 (benchmark & comparator
//! discipline), Q18 (contamination detector).
//!
//! Public benchmark performance is supporting evidence, not the sole release
//! criterion (9.5). Internal tasks carry provenance, creation lineage and
//! contamination status; a signed StudyPlan freezes the protocol before the first
//! optimization run so that later analysis changes cannot inflate the result.
#![forbid(unsafe_code)]

pub mod contamination;
pub mod manifest;
pub mod studyplan;

pub use contamination::{assess, cosine, ngram_jaccard, normalize, normalized_hash, Contamination};
pub use manifest::{BenchmarkManifest, CrossSplitFinding, ManifestEntry};
pub use studyplan::{StoppingRule, StudyPlan};
