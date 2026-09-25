//! Design synthesis and improvement: baseline synthesis, typed edit operators
//! and the DesignHypothesis lifecycle.
//!
//! PRD v11 references: section 11 (design synthesis & improvement), 11.1 (initial
//! design), 11.2 (DesignHypothesis), 11.4 (hierarchical complexity — parallelism
//! and multi-agent delegation), 11.5 (typed edit operators), IR-I7 (pinned nodes).
//!
//! Beyond the single-agent baseline ([`synthesize_single_agent`]) and the level-0/1
//! repair operators, this crate provides the multi-agent layer: a multi-agent
//! synthesizer ([`synthesize_multi_agent`]) and the level-4/5 operators
//! ([`EditOp::SplitParallel`], [`EditOp::AddDelegate`]) that introduce parallel
//! committees and scoped sub-agents. Delegation narrows authority (A2 / IR-I6),
//! enforced when the operator is applied.
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
pub use edit::{apply, AgentSpec, EditOp, PatchError};
pub use hypothesis::{DesignHypothesis, HypothesisResult, DIAGNOSTIC_CONFIDENCE};
pub use synth::{synthesize_multi_agent, synthesize_single_agent, AgentRole};
