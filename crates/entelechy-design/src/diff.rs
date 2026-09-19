//! Structural IR diff (PRD 16.2: IR diff/merge with pinned nodes and resources).
//!
//! Compares two [`Program`]s node-by-node (nodes are keyed by their stable id) and
//! reports additions, removals and changes. Pinned nodes (IR-I7) are flagged so a
//! merge tool can refuse to overwrite them.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use entelechy_ir::{Node, NodeKind, Program};

/// The kind of change to a node between two designs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeKind {
    /// Present only in the new design.
    Added,
    /// Present only in the old design.
    Removed,
    /// Present in both but its behavior changed.
    Changed,
    /// Present in both and identical.
    Unchanged,
}

/// A per-node diff entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeDiff {
    /// The node id.
    pub id: String,
    /// The change.
    pub change: ChangeKind,
    /// Whether the node is pinned in either design (IR-I7): a merge must not
    /// overwrite a pinned node.
    pub pinned: bool,
    /// Short human-readable detail.
    pub detail: String,
}

/// Diff two programs, returning one entry per node id (sorted). Unchanged nodes
/// are included so callers can show full context; filter them out for a summary.
pub fn diff_programs(old: &Program, new: &Program) -> Vec<NodeDiff> {
    let mut old_nodes = BTreeMap::new();
    collect(&old.root, &mut old_nodes);
    let mut new_nodes = BTreeMap::new();
    collect(&new.root, &mut new_nodes);

    let mut ids: Vec<&String> = old_nodes.keys().chain(new_nodes.keys()).collect();
    ids.sort();
    ids.dedup();

    ids.into_iter()
        .map(|id| match (old_nodes.get(id), new_nodes.get(id)) {
            (None, Some(n)) => NodeDiff {
                id: id.clone(),
                change: ChangeKind::Added,
                pinned: n.pinned,
                detail: format!("added {}", kind_name(&n.kind)),
            },
            (Some(o), None) => NodeDiff {
                id: id.clone(),
                change: ChangeKind::Removed,
                pinned: o.pinned,
                detail: format!("removed {}", kind_name(&o.kind)),
            },
            (Some(o), Some(n)) => {
                let changed = o.kind != n.kind || o.pinned != n.pinned;
                NodeDiff {
                    id: id.clone(),
                    change: if changed { ChangeKind::Changed } else { ChangeKind::Unchanged },
                    pinned: o.pinned || n.pinned,
                    detail: if changed {
                        format!("{} -> {}", kind_name(&o.kind), kind_name(&n.kind))
                    } else {
                        kind_name(&n.kind).to_string()
                    },
                }
            }
            (None, None) => unreachable!(),
        })
        .collect()
}

/// Whether a diff contains a change to a pinned node (a merge conflict — IR-I7).
pub fn has_pinned_conflict(diffs: &[NodeDiff]) -> bool {
    diffs
        .iter()
        .any(|d| d.pinned && matches!(d.change, ChangeKind::Changed | ChangeKind::Removed))
}

fn collect<'a>(node: &'a Node, out: &mut BTreeMap<String, &'a Node>) {
    out.insert(node.id.clone(), node);
    for child in children(node) {
        collect(child, out);
    }
}

fn children(node: &Node) -> Vec<&Node> {
    match &node.kind {
        NodeKind::Seq(c) | NodeKind::Par(c) => c.iter().collect(),
        NodeKind::Map { body, .. }
        | NodeKind::Loop { body, .. }
        | NodeKind::Delegate { body, .. } => vec![body.as_ref()],
        NodeKind::Branch { then, els, .. } => vec![then.as_ref(), els.as_ref()],
        _ => vec![],
    }
}

fn kind_name(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Llm(_) => "Llm",
        NodeKind::Tool(_) => "Tool",
        NodeKind::Code(_) => "Code",
        NodeKind::Seq(_) => "Seq",
        NodeKind::Par(_) => "Par",
        NodeKind::Map { .. } => "Map",
        NodeKind::Branch { .. } => "Branch",
        NodeKind::Loop { .. } => "Loop",
        NodeKind::Delegate { .. } => "Delegate",
        NodeKind::Verify(_) => "Verify",
        NodeKind::Mem(_) => "Mem",
        NodeKind::Gate(_) => "Gate",
        NodeKind::Human(_) => "Human",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{apply, synthesize_single_agent, EditOp};
    use entelechy_ir::AuthorityEnvelope;

    #[test]
    fn add_verify_shows_as_added_node() {
        let base = synthesize_single_agent(AuthorityEnvelope::empty(), "m", "{input}");
        let candidate = apply(&base, &EditOp::AddVerify { id: "reply_check".into(), checker: "ok".into() }).unwrap();
        let diffs = diff_programs(&base, &candidate);
        let added: Vec<&NodeDiff> = diffs.iter().filter(|d| d.change == ChangeKind::Added).collect();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].id, "reply_check");
        assert!(!has_pinned_conflict(&diffs));
    }

    #[test]
    fn changed_pinned_node_is_a_conflict() {
        let base = synthesize_single_agent(AuthorityEnvelope::empty(), "m", "{input}");
        // Pin the agent node, then change its model.
        let mut pinned = base.clone();
        if let NodeKind::Seq(c) = &mut pinned.root.kind {
            c[0].pinned = true;
        }
        let changed = apply(&pinned, &EditOp::ChangeModel { node: "agent".into(), model: "big".into() });
        // apply refuses to mutate a pinned node, so simulate an external edit:
        assert!(changed.is_err());
        let mut manual = pinned.clone();
        if let NodeKind::Seq(c) = &mut manual.root.kind {
            if let NodeKind::Llm(l) = &mut c[0].kind {
                l.model = "big".into();
            }
        }
        let diffs = diff_programs(&pinned, &manual);
        assert!(has_pinned_conflict(&diffs));
    }
}
