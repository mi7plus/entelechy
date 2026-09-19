//! Entelechy intermediate representation: node model, value envelope, effect
//! taxonomy, authority and static invariants.
//!
//! PRD v11 references: sections 7 (IR), 5.4 (AuthorityEnvelope), 5.6 (constraint
//! classes), 7.5 (information-flow labels), 7.6 (effect taxonomy).
//!
//! The same typed IR is used during search, replay and production (PRD 3.1). One
//! [`Program`] is a `Design` artifact payload (PRD 5.2): compiler-valid, effects
//! within authority, budgets bounded.
#![forbid(unsafe_code)]

pub mod authority;
pub mod effect;
pub mod invariants;
pub mod node;
pub mod value;

pub use authority::AuthorityEnvelope;
pub use effect::{AttestationLevel, EffectClass, EffectMetadata};
pub use invariants::{validate, CapabilityCatalog, Violation};
pub use node::{
    CodeNode, Condition, GateNode, HumanNode, LlmNode, MemNode, MemOp, Node, NodeKind, Program,
    ToolNode, VerifyNode,
};
pub use value::{Confidentiality, Taint, Value, ValueMeta, VerificationState};

/// Schema id and version for the `Program`/`Design` artifact (PRD 5.9).
pub const DESIGN_SCHEMA_ID: &str = "entelechy.design";

/// Semantic schema version of the IR in this build.
pub const DESIGN_SCHEMA_VERSION: entelechy_artifacts::SchemaVersion =
    entelechy_artifacts::SchemaVersion::new(0, 1, 0);
