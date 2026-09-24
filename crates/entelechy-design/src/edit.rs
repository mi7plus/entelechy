//! Typed edit operators (PRD 11.5). Each operator is a pure, compiler-checkable
//! transformation of a [`Program`]; it never mutates in place, so patches are
//! immutable and diffable. Pinned nodes cannot be mutated (IR-I7).

use serde::{Deserialize, Serialize};

use entelechy_ir::{Node, NodeKind, Program, VerifyNode};

/// A typed edit operator over a design (PRD 11.5). Phase 0 covers the level-0/1
/// operators (model, prompt, parameters, add/remove verify, delete node); higher
/// operators (retrieval, routing, parallelism, delegation) arrive with the
/// hierarchical search unlocks (PRD 11.4).
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
    }
    Ok(next)
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
}
