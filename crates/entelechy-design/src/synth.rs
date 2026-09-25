//! Initial design synthesis (PRD 11.1).
//!
//! The Design Synthesizer emits the simplest design class likely to satisfy the
//! contract. "A single-agent baseline is mandatory whenever the objective can be
//! represented that way" (PRD 11.1) — Entelechy starts here and unlocks
//! complexity only with evidence (PRD principle 4, 11.4).

use entelechy_ir::{AuthorityEnvelope, LlmNode, Node, NodeKind, Program};

/// Synthesize the simplest viable single-agent baseline: one model call under
/// the given authority (PRD 11.1). Complexity is added later only when measured
/// failure justifies it.
///
/// # Example
/// ```
/// use entelechy_design::synthesize_single_agent;
/// use entelechy_ir::AuthorityEnvelope;
///
/// let program =
///     synthesize_single_agent(AuthorityEnvelope::empty(), "mock-small", "resolve: {input}");
/// assert_eq!(program.root.id, "root");
/// ```
pub fn synthesize_single_agent(
    authority: AuthorityEnvelope,
    model: impl Into<String>,
    prompt_template: impl Into<String>,
) -> Program {
    Program::new(
        authority,
        Node::new(
            "root",
            NodeKind::Seq(vec![Node::new(
                "agent",
                NodeKind::Llm(LlmNode {
                    model: model.into(),
                    prompt_template: prompt_template.into(),
                    temperature: 0.0,
                }),
            )]),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_is_single_agent() {
        let p = synthesize_single_agent(AuthorityEnvelope::empty(), "mock", "answer {input}");
        match &p.root.kind {
            NodeKind::Seq(children) => {
                assert_eq!(children.len(), 1);
                assert!(matches!(children[0].kind, NodeKind::Llm(_)));
            }
            _ => panic!("expected a Seq root"),
        }
    }
}
