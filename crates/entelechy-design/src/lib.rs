//! Design synthesis and improvement: baseline synthesis, typed edit operators
//! and the DesignHypothesis lifecycle.
//!
//! PRD v11 references: section 11 (design synthesis & improvement), 11.1 (initial
//! design), 11.2 (DesignHypothesis), 11.5 (typed edit operators), IR-I7 (pinned
//! nodes).
//!
//! The Design Repair Engine is the default improvement path once a viable design
//! exists (PRD 11.3): diagnose → hypothesize → apply the smallest compiler-valid
//! patch → evaluate → retain or revert. This crate provides the patch operators
//! and hypothesis records; the study driver (`entelechy-search`) orchestrates the
//! loop and the runtime + `entelechy-eval` produce the evidence.
#![forbid(unsafe_code)]

pub mod diff;
pub mod edit;
pub mod hypothesis;
pub mod synth;

pub use diff::{diff_programs, has_pinned_conflict, ChangeKind, NodeDiff};
pub use edit::{apply, EditOp, PatchError};
pub use hypothesis::{DesignHypothesis, HypothesisResult, DIAGNOSTIC_CONFIDENCE};
pub use synth::synthesize_single_agent;
