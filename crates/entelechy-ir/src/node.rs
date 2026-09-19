//! IR node model (PRD 7.1) and program structure.
//!
//! The IR is a tree: composite nodes contain their children directly, which
//! keeps serialized IR "human-readable, diffable and version-migratable"
//! (IR-I8). Every node carries a stable `id` so runs can be journaled per node,
//! nodes can be pinned against the design engine (IR-I7), and diffs are stable.
//!
//! Execution is a functional pipeline: each node transforms a single current
//! [`crate::Value`]. This is sufficient for the Phase 0 single-agent slice
//! (PRD 21) and generalizes later.

use serde::{Deserialize, Serialize};

use crate::authority::AuthorityEnvelope;

/// A node in the IR: stable identity plus its kind (PRD 7.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Stable identifier, unique within a program. Used for journaling, pinning
    /// and diffs.
    pub id: String,
    /// If true, the design engine may not mutate this node (IR-I7).
    #[serde(default)]
    pub pinned: bool,
    /// The node's behavior.
    pub kind: NodeKind,
}

impl Node {
    /// Convenience constructor.
    pub fn new(id: impl Into<String>, kind: NodeKind) -> Self {
        Self {
            id: id.into(),
            pinned: false,
            kind,
        }
    }
}

/// The behavior of a node (PRD 7.1).
///
/// Externally tagged (`{"llm": {...}}`) rather than internally tagged: the IR is
/// recursive through `NodeKind`, and internal tagging of a recursive enum makes
/// serde's serializer type-resolution non-terminating. The external form stays
/// human-readable and diffable (IR-I8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeKind {
    /// Model inference with typed structured output. Every call is an egress
    /// effect (PRD 7.5).
    Llm(LlmNode),
    /// Capability invocation through the ToolGateway.
    Tool(ToolNode),
    /// Deterministic computation in an approved runtime.
    Code(CodeNode),
    /// Ordered composition; threads the value through children.
    Seq(Vec<Node>),
    /// Bounded parallel composition (PRD 7.1). Executes children over the same
    /// input; results are collected.
    Par(Vec<Node>),
    /// Apply a subgraph across a collection field of the current value.
    Map {
        /// JSON field (dot path) holding the input collection.
        over: String,
        /// Subgraph applied to each element.
        body: Box<Node>,
    },
    /// Typed conditional control flow.
    Branch {
        /// Condition evaluated against the current value.
        cond: Condition,
        /// Executed when the condition holds.
        then: Box<Node>,
        /// Executed otherwise.
        #[serde(rename = "else")]
        els: Box<Node>,
    },
    /// Bounded iteration with an explicit termination contract (IR-I4).
    Loop {
        /// Subgraph run each iteration.
        body: Box<Node>,
        /// Hard upper bound on iterations (IR-I4).
        max_iters: u32,
        /// Stop early when this condition becomes true.
        until: Condition,
    },
    /// Scoped sub-agent with narrowed authority (A2 / IR-I6).
    Delegate {
        /// The child authority; must be a subset of the parent (IR-I6).
        authority: AuthorityEnvelope,
        /// The delegated subgraph.
        body: Box<Node>,
    },
    /// Evidence-producing verification step.
    Verify(VerifyNode),
    /// Read/write memory according to policy.
    Mem(MemNode),
    /// Policy/evidence/approval boundary before an authority transition (PRD 7.3).
    Gate(GateNode),
    /// Explicit human decision or data input (carries a timeout — PRD 8.6).
    Human(HumanNode),
}

/// A model inference node (PRD 7.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LlmNode {
    /// Logical model identity requested (resolved by the ModelGateway).
    pub model: String,
    /// Prompt template. `{input}` is replaced with the current value's payload.
    pub prompt_template: String,
    /// Sampling temperature.
    #[serde(default)]
    pub temperature: f64,
}

/// A tool invocation node (PRD 7.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolNode {
    /// Capability name; must be granted by the AuthorityEnvelope (IR-I2).
    pub capability: String,
    /// Static arguments merged with the current value.
    #[serde(default)]
    pub args: serde_json::Value,
}

/// A deterministic code node (PRD 7.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeNode {
    /// Registered pure function name in the approved runtime.
    pub function: String,
}

/// A verification node (PRD 7.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerifyNode {
    /// Registered checker name.
    pub checker: String,
}

/// A memory node (PRD 7.1, 12).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemNode {
    /// Whether this reads or writes memory.
    pub op: MemOp,
    /// Memory key.
    pub key: String,
}

/// Memory operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemOp {
    /// Read a value from memory.
    Read,
    /// Write the current value to memory.
    Write,
}

/// A gate node (PRD 7.3): evaluates a condition against evidence/policy and
/// controls whether execution can cross into a more consequential effect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GateNode {
    /// Named policy this gate consults.
    pub policy: String,
    /// Condition that must hold for the gate to open.
    pub condition: Condition,
    /// Whether crossing this gate requires human approval.
    #[serde(default)]
    pub requires_approval: bool,
}

/// A human node (PRD 8.6). An expired human decision is a typed timeout, never
/// implicit approval (RS-2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HumanNode {
    /// Prompt shown to the human.
    pub prompt: String,
    /// Timeout in seconds before the decision expires as a typed timeout.
    pub timeout_secs: u64,
}

/// A typed condition over the current value (PRD 7.1 Branch / 7.3 Gate).
///
/// Externally tagged for the same reason as [`NodeKind`]: `Condition` recurses
/// through `Not`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Condition {
    /// Always true.
    Always,
    /// Never true.
    Never,
    /// The named JSON field (dot path) is present and truthy.
    Truthy {
        /// Dot-path field, e.g. `result.ok`.
        field: String,
    },
    /// The named field equals the given JSON value.
    Equals {
        /// Dot-path field.
        field: String,
        /// Expected value.
        value: serde_json::Value,
    },
    /// The current value is tainted (integrity check for gates — IR-I3).
    IsTainted,
    /// Logical negation.
    Not(Box<Condition>),
}

/// A complete IR program (PRD 5.2 Design artifact payload).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Program {
    /// The system authority envelope (A1).
    pub authority: AuthorityEnvelope,
    /// The root node.
    pub root: Node,
}

impl Program {
    /// Construct a program.
    pub fn new(authority: AuthorityEnvelope, root: Node) -> Self {
        Self { authority, root }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_serde_roundtrip() {
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "classify",
                        NodeKind::Llm(LlmNode {
                            model: "mock".into(),
                            prompt_template: "classify {input}".into(),
                            temperature: 0.0,
                        }),
                    ),
                    Node::new("check", NodeKind::Verify(VerifyNode { checker: "ok".into() })),
                ]),
            ),
        );
        let json = serde_json::to_string(&prog).unwrap();
        let back: Program = serde_json::from_str(&json).unwrap();
        assert_eq!(prog, back);
    }
}
