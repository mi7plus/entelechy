//! Initial design synthesis (PRD 11.1).
//!
//! The Design Synthesizer emits the simplest design class likely to satisfy the
//! contract. "A single-agent baseline is mandatory whenever the objective can be
//! represented that way" (PRD 11.1) — Entelechy starts here and unlocks
//! complexity only with evidence (PRD principle 4, 11.4).

use entelechy_ir::{AuthorityEnvelope, LlmNode, Node, NodeKind, Program};

/// One scoped sub-agent role in a multi-agent design (PRD 11.4 level 5).
#[derive(Clone, Debug, PartialEq)]
pub struct AgentRole {
    /// The sub-agent's node id.
    pub id: String,
    /// The model the sub-agent runs.
    pub model: String,
    /// The sub-agent's prompt template (`{input}` is substituted).
    pub prompt_template: String,
    /// The narrowed authority granted to this sub-agent. Must be a subset of the
    /// coordinator authority (A2 / IR-I6); callers should pass an envelope that is
    /// a subset, and [`crate::edit::apply`]/`validate` enforce it.
    pub authority: AuthorityEnvelope,
}

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

/// Synthesize a multi-agent design: a coordinator that runs the given sub-agents
/// in parallel, each as a `Delegate` with its own narrowed authority (PRD 11.4
/// level 5, A2 / IR-I6). This is the multi-agent counterpart to
/// [`synthesize_single_agent`] — reached only once the Architecture Explorer
/// unlocks delegation on evidence (PRD 11.4), never as the first design (11.1).
///
/// The `coordinator_authority` is the outer envelope; each role's authority must
/// be a subset of it. The result is structurally valid when that holds; callers
/// should run `entelechy_ir::validate` to confirm (IR-I6).
///
/// # Example
/// ```
/// use entelechy_design::synth::{synthesize_multi_agent, AgentRole};
/// use entelechy_ir::{AuthorityEnvelope, NodeKind};
///
/// let mut coord = AuthorityEnvelope::empty();
/// coord.capabilities.insert("read_crm".into());
/// let mut sub = AuthorityEnvelope::empty();
/// sub.capabilities.insert("read_crm".into());
///
/// let program = synthesize_multi_agent(
///     coord,
///     &[AgentRole {
///         id: "researcher".into(),
///         model: "mock".into(),
///         prompt_template: "research: {input}".into(),
///         authority: sub,
///     }],
/// );
/// assert!(matches!(program.root.kind, NodeKind::Par(_)));
/// ```
pub fn synthesize_multi_agent(
    coordinator_authority: AuthorityEnvelope,
    roles: &[AgentRole],
) -> Program {
    let agents: Vec<Node> = roles
        .iter()
        .map(|role| {
            Node::new(
                role.id.clone(),
                NodeKind::Delegate {
                    authority: role.authority.clone(),
                    body: Box::new(Node::new(
                        format!("{}_llm", role.id),
                        NodeKind::Llm(LlmNode {
                            model: role.model.clone(),
                            prompt_template: role.prompt_template.clone(),
                            temperature: 0.0,
                        }),
                    )),
                },
            )
        })
        .collect();
    Program::new(
        coordinator_authority,
        Node::new("root", NodeKind::Par(agents)),
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

    #[test]
    fn multi_agent_is_parallel_delegates_and_valid() {
        let mut coord = AuthorityEnvelope::empty();
        coord.capabilities.insert("read_crm".into());
        coord.capabilities.insert("summarize".into());
        let mut a1 = AuthorityEnvelope::empty();
        a1.capabilities.insert("read_crm".into());
        let mut a2 = AuthorityEnvelope::empty();
        a2.capabilities.insert("summarize".into());

        let p = synthesize_multi_agent(
            coord,
            &[
                AgentRole {
                    id: "researcher".into(),
                    model: "mock".into(),
                    prompt_template: "research: {input}".into(),
                    authority: a1,
                },
                AgentRole {
                    id: "summarizer".into(),
                    model: "mock".into(),
                    prompt_template: "summarize: {input}".into(),
                    authority: a2,
                },
            ],
        );
        match &p.root.kind {
            NodeKind::Par(children) => {
                assert_eq!(children.len(), 2);
                assert!(children
                    .iter()
                    .all(|c| matches!(c.kind, NodeKind::Delegate { .. })));
            }
            _ => panic!("expected a Par root"),
        }
        // Each delegate narrows the coordinator authority (IR-I6).
        let catalog: std::collections::HashMap<String, entelechy_ir::EffectMetadata> =
            std::collections::HashMap::new();
        assert!(entelechy_ir::validate(&p, &catalog)
            .iter()
            .all(|v| v.code != "IR-I6"));
    }
}
