//! Static compiler invariants (PRD 7.4).
//!
//! [`validate`] walks a [`Program`] and reports violations of the machine-checkable
//! invariants. These are *structural* constraints (PRD 5.6): a failed proof makes
//! the design infeasible, it is never a scalar penalty (PRD 9.2).
//!
//! Implemented in this slice:
//! - **IR-I2**: every effect is declared and within granted authority.
//! - **IR-I3**: tainted values cannot reach privileged effects without a Gate.
//! - **IR-I4**: loops have bounded iteration counts.
//! - **IR-I6**: delegation authority is monotonically narrowing.
//! - **IR-I10**: every capability resolves effect metadata; unknown metadata is
//!   treated conservatively (external + irreversible) per CD-7.
//!
//! Deferred (need the type system / budget model / provider routing): IR-I1,
//! IR-I5, IR-I7 (enforced by the design engine), IR-I8, IR-I9.

use std::collections::HashMap;

use crate::authority::AuthorityEnvelope;
use crate::effect::EffectMetadata;
use crate::node::{Node, NodeKind};

/// Resolves effect metadata for a capability name (PRD 7.6, IR-I10).
pub trait CapabilityCatalog {
    /// Look up declared effect metadata, if any.
    fn effect(&self, capability: &str) -> Option<EffectMetadata>;
}

impl CapabilityCatalog for HashMap<String, EffectMetadata> {
    fn effect(&self, capability: &str) -> Option<EffectMetadata> {
        self.get(capability).cloned()
    }
}

/// A single invariant violation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// Invariant identifier, e.g. `"IR-I3"`.
    pub code: &'static str,
    /// Node at which the violation was detected.
    pub node_id: String,
    /// Human-readable explanation.
    pub message: String,
}

/// Validate a program against the static invariants. An empty result means the
/// program is structurally feasible for the checked invariants.
pub fn validate<C: CapabilityCatalog>(program: &crate::Program, catalog: &C) -> Vec<Violation> {
    let mut v = Vec::new();
    let mut ctx = Ctx {
        catalog,
        violations: &mut v,
    };
    walk(&mut ctx, &program.root, &program.authority, false);
    v
}

struct Ctx<'a, C: CapabilityCatalog> {
    catalog: &'a C,
    violations: &'a mut Vec<Violation>,
}

/// Walk a node, returning whether the value *leaving* it may be tainted.
///
/// `tainted_in` is whether the value entering the node may be tainted.
fn walk<C: CapabilityCatalog>(
    ctx: &mut Ctx<C>,
    node: &Node,
    authority: &AuthorityEnvelope,
    tainted_in: bool,
) -> bool {
    match &node.kind {
        NodeKind::Llm(_) => {
            // Model output is external content and enters as tainted (PRD 5.8, 7.5).
            true
        }
        NodeKind::Code(_) => tainted_in,
        NodeKind::Mem(_) => tainted_in,
        NodeKind::Human(_) => tainted_in,
        NodeKind::Verify(_) => {
            // A Verify step produces evidence but does not by itself declassify
            // integrity in this slice.
            tainted_in
        }
        NodeKind::Tool(tool) => {
            // IR-I2 / IR-I10: resolve effect metadata (conservative if unknown).
            let meta = ctx
                .catalog
                .effect(&tool.capability)
                .unwrap_or_else(|| EffectMetadata::conservative_default(&tool.capability));

            // IR-I2: capability must be granted by the authority envelope.
            if !authority.allows_capability(&tool.capability) {
                ctx.violations.push(Violation {
                    code: "IR-I2",
                    node_id: node.id.clone(),
                    message: format!(
                        "capability '{}' is not granted by the AuthorityEnvelope",
                        tool.capability
                    ),
                });
            }

            // IR-I3: a tainted value must not reach a privileged (consequential)
            // effect without an intervening Gate.
            if tainted_in && meta.class.is_consequential() {
                ctx.violations.push(Violation {
                    code: "IR-I3",
                    node_id: node.id.clone(),
                    message: format!(
                        "tainted value reaches consequential effect '{}' ({:?}) without a Gate",
                        tool.capability, meta.class
                    ),
                });
            }

            // Tool output is external content: conservatively tainted.
            true
        }
        NodeKind::Gate(_) => {
            // A Gate is the sanctioned taint -> privilege boundary (IR-I3, PRD 7.3):
            // downstream of an opened gate the value is no longer treated as an
            // ungated tainted input.
            false
        }
        NodeKind::Seq(children) => {
            let mut tainted = tainted_in;
            for child in children {
                tainted = walk(ctx, child, authority, tainted);
            }
            tainted
        }
        NodeKind::Par(children) => {
            let mut any_tainted = false;
            for child in children {
                any_tainted |= walk(ctx, child, authority, tainted_in);
            }
            any_tainted
        }
        NodeKind::Map { body, .. } => walk(ctx, body, authority, tainted_in),
        NodeKind::Branch { then, els, .. } => {
            let t = walk(ctx, then, authority, tainted_in);
            let e = walk(ctx, els, authority, tainted_in);
            t || e
        }
        NodeKind::Loop {
            body,
            max_iters,
            ..
        } => {
            // IR-I4: loops must be bounded.
            if *max_iters == 0 {
                ctx.violations.push(Violation {
                    code: "IR-I4",
                    node_id: node.id.clone(),
                    message: "loop max_iters must be greater than zero".into(),
                });
            }
            // Conservatively analyze one iteration, feeding output back once.
            let once = walk(ctx, body, authority, tainted_in);
            walk(ctx, body, authority, once)
        }
        NodeKind::Delegate { authority: child, body } => {
            // IR-I6 / A2: delegated authority must narrow.
            if !child.is_subset_of(authority) {
                ctx.violations.push(Violation {
                    code: "IR-I6",
                    node_id: node.id.clone(),
                    message: "delegated authority is not a subset of the parent authority".into(),
                });
            }
            walk(ctx, body, child, tainted_in)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::{AttestationLevel, EffectClass, EffectMetadata};
    use crate::node::{GateNode, LlmNode, ToolNode, Condition};
    use crate::Program;
    use std::collections::HashMap;

    fn catalog() -> HashMap<String, EffectMetadata> {
        let mut c = HashMap::new();
        c.insert(
            "send_payment".to_string(),
            EffectMetadata {
                class: EffectClass::Irreversible,
                idempotent: false,
                reversible: false,
                dry_run_supported: false,
                read_back_supported: false,
                attestation: AttestationLevel::OperatorAttested,
                operation_key_namespace: "pay".into(),
            },
        );
        c
    }

    fn authority(caps: &[&str]) -> AuthorityEnvelope {
        AuthorityEnvelope {
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn ungranted_capability_is_flagged() {
        let prog = Program::new(
            authority(&[]),
            Node::new(
                "pay",
                NodeKind::Tool(ToolNode {
                    capability: "send_payment".into(),
                    args: serde_json::Value::Null,
                }),
            ),
        );
        let vio = validate(&prog, &catalog());
        assert!(vio.iter().any(|v| v.code == "IR-I2"));
    }

    #[test]
    fn tainted_reaching_effect_without_gate_is_flagged() {
        // Llm (tainted) -> payment tool, no gate: IR-I3.
        let prog = Program::new(
            authority(&["send_payment"]),
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "read",
                        NodeKind::Llm(LlmNode {
                            model: "mock".into(),
                            prompt_template: "{input}".into(),
                            temperature: 0.0,
                        }),
                    ),
                    Node::new(
                        "pay",
                        NodeKind::Tool(ToolNode {
                            capability: "send_payment".into(),
                            args: serde_json::Value::Null,
                        }),
                    ),
                ]),
            ),
        );
        let vio = validate(&prog, &catalog());
        assert!(vio.iter().any(|v| v.code == "IR-I3"), "{vio:?}");
    }

    #[test]
    fn gate_before_effect_clears_taint() {
        let prog = Program::new(
            authority(&["send_payment"]),
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "read",
                        NodeKind::Llm(LlmNode {
                            model: "mock".into(),
                            prompt_template: "{input}".into(),
                            temperature: 0.0,
                        }),
                    ),
                    Node::new(
                        "gate",
                        NodeKind::Gate(GateNode {
                            policy: "approve_payments".into(),
                            condition: Condition::Always,
                            requires_approval: true,
                        }),
                    ),
                    Node::new(
                        "pay",
                        NodeKind::Tool(ToolNode {
                            capability: "send_payment".into(),
                            args: serde_json::Value::Null,
                        }),
                    ),
                ]),
            ),
        );
        let vio = validate(&prog, &catalog());
        assert!(vio.is_empty(), "expected no violations, got {vio:?}");
    }

    #[test]
    fn delegation_must_narrow() {
        let parent = authority(&["read_crm"]);
        let wider = authority(&["read_crm", "send_payment"]);
        let prog = Program::new(
            parent,
            Node::new(
                "d",
                NodeKind::Delegate {
                    authority: wider,
                    body: Box::new(Node::new("noop", NodeKind::Code(crate::node::CodeNode {
                        function: "id".into(),
                    }))),
                },
            ),
        );
        let vio = validate(&prog, &catalog());
        assert!(vio.iter().any(|v| v.code == "IR-I6"));
    }
}
