//! Failure intelligence: ontology, clustering and causal evidence.
//!
//! PRD v11 references: section 10 (failure intelligence), 10.1 (failure
//! ontology), 10.2 (failure analyzer), 11.4 (complexity-level eligibility), Q3
//! (uncertainty → budget allocation), 22 (ontology versioning).
//!
//! The analyzer clusters failed traces, assigns failure classes with calibrated
//! confidence, distinguishes system from evaluation failure, and keeps an
//! unresolved-cause state rather than forcing a confident diagnosis. Its output
//! feeds the Design Repair Engine (`entelechy-design`), which turns a diagnosis
//! into a DesignHypothesis.
#![forbid(unsafe_code)]

pub mod analyzer;
pub mod ontology;

pub use analyzer::{
    allocate_budget, analyze, ClassProbability, FailureCluster, FailureObservation, Symptom,
    DIAGNOSTIC_CONFIDENCE, HIGH_ENTROPY_RESERVE,
};
pub use ontology::{class, Family, FailureClass, ONTOLOGY, ONTOLOGY_VERSION};
