//! Typed edit operators (PRD 11.5). Each operator is a pure, compiler-checkable
//! transformation of a [`Program`]; it never mutates in place, so patches are
//! immutable and diffable. Pinned nodes cannot be mutated (IR-I7).

use serde::{Deserialize, Serialize};

use entelechy_ir::{AuthorityEnvelope, LlmNode, Node, NodeKind, Program, VerifyNode};

/// One agent in a parallel decomposition (PRD 11.4 level 4 / 11.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Node id for this agent's `Llm` node (must be unique within the design).
    pub id: String,
    /// The agent's specialized prompt template (`{input}` is substituted).
    pub prompt_template: String,
}

/// A typed edit operator over a design (PRD 11.5). The level-0/1 operators (model,
/// prompt, parameters, add/remove verify, delete node) cover single-agent repair;
/// the level-4/5 operators ([`EditOp::SplitParallel`], [`EditOp::AddDelegate`])
/// introduce parallel and multi-agent structure, and are only reached once the
/// Architecture Explorer unlocks that complexity level on evidence (PRD 11.4, Q4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EditOp {
    /// Change the model of an `Llm` node.
    ChangeModel {
        /// Target node id.
        node: String,
        /// New model identity.
        model: String,
    },
    /// Replace the prompt template of an `Llm` node.
    MutatePrompt {
        /// Target node id.
        node: String,
        /// New template.
        template: String,
    },
    /// Set the sampling temperature of an `Llm` node.
    SetTemperature {
        /// Target node id.
        node: String,
        /// New temperature.
        temperature: f64,
    },
    /// Append a `Verify` node to the end of the root `Seq` (add a verification
    /// step — PRD 11.5).
    AddVerify {
        /// New node id.
        id: String,
        /// Checker name.
        checker: String,
    },
    /// Delete a node with no measured contribution (PRD 11.5 pruning).
    DeleteNode {
        /// Target node id.
        node: String,
    },
    /// Level 4 (parallelism): replace an `Llm` node with a bounded `Par` of
    /// specialized agent `Llm` nodes over the same input (a committee/ensemble).
    /// The node keeps its id and becomes the `Par`; results are collected as an
    /// array (PRD 7.1 Par semantics, 11.4 level 4).
    SplitParallel {
        /// The `Llm` node to fan out.
        node: String,
        /// The agents to run in parallel (at least two; the parent model is reused).
        agents: Vec<AgentSpec>,
    },
    /// Level 5 (delegation / multi-agent): wrap the subgraph rooted at `node` in a
    /// `Delegate` with narrowed authority — a scoped sub-agent (PRD 7.1, 11.4 level
    /// 5). The delegated authority must be a subset of the authority in force at
    /// the wrap site (A2 / IR-I6); widening is rejected.
    AddDelegate {
        /// The subgraph to place under a scoped authority.
        node: String,
        /// The new `Delegate` wrapper node's id.
        wrapper_id: String,
        /// The narrowed authority for the sub-agent (must be a subset of the
        /// enclosing authority — A2 / IR-I6).
        authority: AuthorityEnvelope,
    },
}

/// Errors from applying an [`EditOp`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PatchError {
    /// No node with the given id exists.
    #[error("node '{0}' not found")]
    NotFound(String),
    /// The node is pinned and cannot be mutated (IR-I7).
    #[error("node '{0}' is pinned and cannot be mutated (IR-I7)")]
    Pinned(String),
    /// The operator does not apply to this node's kind.
    #[error("operator does not apply to node '{0}' of this kind")]
    WrongKind(String),
    /// The edit would produce a structurally invalid program.
    #[error("edit is structurally invalid: {0}")]
    Structural(String),
}

/// Apply an edit operator, returning a new program (the original is unchanged).
pub fn apply(program: &Program, op: &EditOp) -> Result<Program, PatchError> {
    let mut next = program.clone();
    match op {
        EditOp::ChangeModel { node, model } => {
            with_llm(&mut next.root, node, |llm| llm.model = model.clone())?;
        }
        EditOp::MutatePrompt { node, template } => {
            with_llm(&mut next.root, node, |llm| {
                llm.prompt_template = template.clone()
            })?;
        }
        EditOp::SetTemperature { node, temperature } => {
            with_llm(&mut next.root, node, |llm| llm.temperature = *temperature)?;
        }
        EditOp::AddVerify { id, checker } => match &mut next.root.kind {
            NodeKind::Seq(children) => children.push(Node::new(
                id.clone(),
                NodeKind::Verify(VerifyNode {
                    checker: checker.clone(),
                }),
            )),
            _ => {
                return Err(PatchError::Structural(
                    "AddVerify requires a Seq root".into(),
                ))
            }
        },
        EditOp::DeleteNode { node } => {
            if next.root.id == *node {
                return Err(PatchError::Structural("cannot delete the root node".into()));
            }
            let removed = delete_node(&mut next.root, node)?;
            if !removed {
                return Err(PatchError::NotFound(node.clone()));
            }
        }
        EditOp::SplitParallel { node, agents } => {
            if agents.len() < 2 {
                return Err(PatchError::Structural(
                    "SplitParallel requires at least two agents".into(),
                ));
            }
            let target =
                find_node_mut(&mut next.root, node).ok_or(PatchError::NotFound(node.clone()))?;
            if target.pinned {
                return Err(PatchError::Pinned(node.clone()));
            }
            let NodeKind::Llm(base) = &target.kind else {
                return Err(PatchError::WrongKind(node.clone()));
            };
            let base = base.clone();
            let children = agents
                .iter()
                .map(|a| {
                    Node::new(
                        a.id.clone(),
                        NodeKind::Llm(LlmNode {
                            model: base.model.clone(),
                            prompt_template: a.prompt_template.clone(),
                            temperature: base.temperature,
                        }),
                    )
                })
                .collect();
            target.kind = NodeKind::Par(children);
        }
        EditOp::AddDelegate {
            node,
            wrapper_id,
            authority,
        } => {
            // A2 / IR-I6: the sub-agent's authority must narrow the authority in
            // force at the wrap site. Compute it before mutating and fail closed.
            let enclosing = authority_at(&next.root, &next.authority, node)
                .ok_or(PatchError::NotFound(node.clone()))?;
            if !authority.is_subset_of(&enclosing) {
                return Err(PatchError::Structural(format!(
                    "delegated authority for '{node}' is not a subset of the enclosing \
                     authority (A2 / IR-I6)"
                )));
            }
            let target =
                find_node_mut(&mut next.root, node).ok_or(PatchError::NotFound(node.clone()))?;
            if target.pinned {
                return Err(PatchError::Pinned(node.clone()));
            }
            // Wrap the subgraph in a Delegate, preserving the body's id.
            let placeholder = Node::new("", NodeKind::Seq(Vec::new()));
            let body = std::mem::replace(target, placeholder);
            *target = Node::new(
                wrapper_id.clone(),
                NodeKind::Delegate {
                    authority: authority.clone(),
                    body: Box::new(body),
                },
            );
        }
    }
    Ok(next)
}

/// The authority in force at the node with id `target`: the program authority,
/// narrowed by every `Delegate` ancestor on the path to it (A2 / IR-I6). `None`
/// if no such node exists.
fn authority_at(
    node: &Node,
    current: &AuthorityEnvelope,
    target: &str,
) -> Option<AuthorityEnvelope> {
    if node.id == target {
        return Some(current.clone());
    }
    match &node.kind {
        NodeKind::Seq(c) | NodeKind::Par(c) => {
            c.iter().find_map(|ch| authority_at(ch, current, target))
        }
        NodeKind::Map { body, .. } | NodeKind::Loop { body, .. } => {
            authority_at(body, current, target)
        }
        NodeKind::Delegate { authority, body } => authority_at(body, authority, target),
        NodeKind::Branch { then, els, .. } => {
            authority_at(then, current, target).or_else(|| authority_at(els, current, target))
        }
        _ => None,
    }
}

/// Find an `Llm` node by id and apply `f`, honoring the pin guard (IR-I7).
fn with_llm(
    node: &mut Node,
    id: &str,
    f: impl FnOnce(&mut entelechy_ir::LlmNode),
) -> Result<(), PatchError> {
    match find_node_mut(node, id) {
        Some(n) if n.pinned => Err(PatchError::Pinned(id.to_string())),
        Some(n) => match &mut n.kind {
            NodeKind::Llm(llm) => {
                f(llm);
                Ok(())
            }
            _ => Err(PatchError::WrongKind(id.to_string())),
        },
        None => Err(PatchError::NotFound(id.to_string())),
    }
}

/// Depth-first search for a node by id, returning a mutable reference.
fn find_node_mut<'a>(node: &'a mut Node, id: &str) -> Option<&'a mut Node> {
    if node.id == id {
        return Some(node);
    }
    for child in children_mut(node) {
        if let Some(found) = find_node_mut(child, id) {
            return Some(found);
        }
    }
    None
}

/// Delete a node by id from any `Seq`/`Par` child list. Returns whether removed.
/// A pinned node cannot be deleted (IR-I7).
fn delete_node(node: &mut Node, id: &str) -> Result<bool, PatchError> {
    match &mut node.kind {
        NodeKind::Seq(children) | NodeKind::Par(children) => {
            if let Some(pos) = children.iter().position(|c| c.id == id) {
                if children[pos].pinned {
                    return Err(PatchError::Pinned(id.to_string()));
                }
                children.remove(pos);
                return Ok(true);
            }
            for child in children.iter_mut() {
                if delete_node(child, id)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => {
            for child in children_mut(node) {
                if delete_node(child, id)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

/// Mutable references to a node's direct children (for traversal).
fn children_mut(node: &mut Node) -> Vec<&mut Node> {
    match &mut node.kind {
        NodeKind::Seq(c) | NodeKind::Par(c) => c.iter_mut().collect(),
        NodeKind::Map { body, .. }
        | NodeKind::Loop { body, .. }
        | NodeKind::Delegate { body, .. } => vec![body.as_mut()],
        NodeKind::Branch { then, els, .. } => vec![then.as_mut(), els.as_mut()],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_ir::{AuthorityEnvelope, CodeNode, LlmNode, Node, NodeKind, Program};

    fn base() -> Program {
        Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "llm",
                        NodeKind::Llm(LlmNode {
                            model: "small".into(),
                            prompt_template: "{input}".into(),
                            temperature: 0.0,
                        }),
                    ),
                    Node::new(
                        "code",
                        NodeKind::Code(CodeNode {
                            function: "id".into(),
                        }),
                    ),
                ]),
            ),
        )
    }

    #[test]
    fn change_model_produces_new_program() {
        let p = base();
        let q = apply(
            &p,
            &EditOp::ChangeModel {
                node: "llm".into(),
                model: "large".into(),
            },
        )
        .unwrap();
        // Original unchanged (immutability).
        if let NodeKind::Seq(c) = &p.root.kind {
            if let NodeKind::Llm(l) = &c[0].kind {
                assert_eq!(l.model, "small");
            }
        }
        if let NodeKind::Seq(c) = &q.root.kind {
            if let NodeKind::Llm(l) = &c[0].kind {
                assert_eq!(l.model, "large");
            }
        }
    }

    #[test]
    fn pinned_node_cannot_be_mutated() {
        let mut p = base();
        if let NodeKind::Seq(c) = &mut p.root.kind {
            c[0].pinned = true;
        }
        let err = apply(
            &p,
            &EditOp::MutatePrompt {
                node: "llm".into(),
                template: "x".into(),
            },
        );
        assert_eq!(err, Err(PatchError::Pinned("llm".into())));
    }

    #[test]
    fn wrong_kind_is_rejected() {
        let p = base();
        let err = apply(
            &p,
            &EditOp::ChangeModel {
                node: "code".into(),
                model: "m".into(),
            },
        );
        assert_eq!(err, Err(PatchError::WrongKind("code".into())));
    }

    #[test]
    fn add_verify_and_delete_node() {
        let p = base();
        let q = apply(
            &p,
            &EditOp::AddVerify {
                id: "v".into(),
                checker: "ok".into(),
            },
        )
        .unwrap();
        if let NodeKind::Seq(c) = &q.root.kind {
            assert_eq!(c.len(), 3);
        }
        let r = apply(
            &q,
            &EditOp::DeleteNode {
                node: "code".into(),
            },
        )
        .unwrap();
        if let NodeKind::Seq(c) = &r.root.kind {
            assert_eq!(c.len(), 2);
            assert!(!c.iter().any(|n| n.id == "code"));
        }
    }

    #[test]
    fn missing_node_not_found() {
        let p = base();
        assert_eq!(
            apply(
                &p,
                &EditOp::DeleteNode {
                    node: "ghost".into()
                }
            ),
            Err(PatchError::NotFound("ghost".into()))
        );
    }

    #[test]
    fn split_parallel_fans_an_llm_into_a_committee() {
        let p = base();
        let q = apply(
            &p,
            &EditOp::SplitParallel {
                node: "llm".into(),
                agents: vec![
                    AgentSpec {
                        id: "fast".into(),
                        prompt_template: "quick: {input}".into(),
                    },
                    AgentSpec {
                        id: "careful".into(),
                        prompt_template: "careful: {input}".into(),
                    },
                ],
            },
        )
        .unwrap();
        // The node kept its id and became a Par of two Llm agents reusing the model.
        let target = find(&q.root, "llm").unwrap();
        match &target.kind {
            NodeKind::Par(children) => {
                assert_eq!(children.len(), 2);
                assert_eq!(children[0].id, "fast");
                if let NodeKind::Llm(l) = &children[0].kind {
                    assert_eq!(l.model, "small"); // parent model reused
                    assert_eq!(l.prompt_template, "quick: {input}");
                } else {
                    panic!("expected Llm");
                }
            }
            other => panic!("expected Par, got {other:?}"),
        }
    }

    #[test]
    fn split_parallel_requires_two_agents_and_an_llm() {
        let p = base();
        assert!(matches!(
            apply(
                &p,
                &EditOp::SplitParallel {
                    node: "llm".into(),
                    agents: vec![AgentSpec {
                        id: "only".into(),
                        prompt_template: "{input}".into()
                    }],
                }
            ),
            Err(PatchError::Structural(_))
        ));
        // Not an Llm node.
        assert_eq!(
            apply(
                &p,
                &EditOp::SplitParallel {
                    node: "code".into(),
                    agents: vec![
                        AgentSpec {
                            id: "a".into(),
                            prompt_template: "{input}".into()
                        },
                        AgentSpec {
                            id: "b".into(),
                            prompt_template: "{input}".into()
                        },
                    ],
                }
            ),
            Err(PatchError::WrongKind("code".into()))
        );
    }

    #[test]
    fn add_delegate_wraps_with_narrowed_authority() {
        // Program authority grants two capabilities; delegate to a subset.
        let mut p = base();
        p.authority.capabilities.insert("read_crm".into());
        p.authority.capabilities.insert("draft_reply".into());

        let mut narrowed = AuthorityEnvelope::empty();
        narrowed.capabilities.insert("read_crm".into());

        let q = apply(
            &p,
            &EditOp::AddDelegate {
                node: "llm".into(),
                wrapper_id: "sub_agent".into(),
                authority: narrowed.clone(),
            },
        )
        .unwrap();

        let wrapper = find(&q.root, "sub_agent").unwrap();
        match &wrapper.kind {
            NodeKind::Delegate { authority, body } => {
                assert_eq!(authority, &narrowed);
                assert_eq!(body.id, "llm"); // body id preserved
            }
            other => panic!("expected Delegate, got {other:?}"),
        }
        // The result is structurally valid (IR-I6 holds).
        let catalog: std::collections::HashMap<String, entelechy_ir::EffectMetadata> =
            std::collections::HashMap::new();
        assert!(entelechy_ir::validate(&q, &catalog)
            .iter()
            .all(|v| v.code != "IR-I6"));
    }

    #[test]
    fn add_delegate_rejects_authority_widening() {
        // Program grants only read_crm; a delegate cannot add draft_reply (A2).
        let mut p = base();
        p.authority.capabilities.insert("read_crm".into());

        let mut widened = AuthorityEnvelope::empty();
        widened.capabilities.insert("read_crm".into());
        widened.capabilities.insert("draft_reply".into()); // not in parent

        assert!(matches!(
            apply(
                &p,
                &EditOp::AddDelegate {
                    node: "llm".into(),
                    wrapper_id: "w".into(),
                    authority: widened,
                }
            ),
            Err(PatchError::Structural(_))
        ));
    }

    #[test]
    fn nested_delegate_cannot_rewiden_beyond_its_parent() {
        // root(auth: a,b) -> Delegate d(auth: a) -> llm. Wrapping llm with {b} would
        // widen beyond the enclosing delegate authority {a}, and must be rejected
        // even though {b} is within the program authority.
        let mut auth = AuthorityEnvelope::empty();
        auth.capabilities.insert("a".into());
        auth.capabilities.insert("b".into());
        let mut inner = AuthorityEnvelope::empty();
        inner.capabilities.insert("a".into());

        let prog = Program::new(
            auth,
            Node::new(
                "d",
                NodeKind::Delegate {
                    authority: inner,
                    body: Box::new(Node::new(
                        "llm",
                        NodeKind::Llm(LlmNode {
                            model: "m".into(),
                            prompt_template: "{input}".into(),
                            temperature: 0.0,
                        }),
                    )),
                },
            ),
        );
        let mut rewiden = AuthorityEnvelope::empty();
        rewiden.capabilities.insert("b".into()); // in program, not in enclosing {a}
        assert!(matches!(
            apply(
                &prog,
                &EditOp::AddDelegate {
                    node: "llm".into(),
                    wrapper_id: "w".into(),
                    authority: rewiden,
                }
            ),
            Err(PatchError::Structural(_))
        ));
    }

    /// Depth-first find by id (test helper).
    fn find<'a>(node: &'a Node, id: &str) -> Option<&'a Node> {
        if node.id == id {
            return Some(node);
        }
        match &node.kind {
            NodeKind::Seq(c) | NodeKind::Par(c) => c.iter().find_map(|n| find(n, id)),
            NodeKind::Map { body, .. }
            | NodeKind::Loop { body, .. }
            | NodeKind::Delegate { body, .. } => find(body, id),
            NodeKind::Branch { then, els, .. } => find(then, id).or_else(|| find(els, id)),
            _ => None,
        }
    }
}
