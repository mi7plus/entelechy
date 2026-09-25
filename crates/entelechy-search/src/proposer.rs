//! Operator proposal: turn a diagnosed failure into the next design mutation,
//! escalating complexity on evidence (PRD 11.3, 11.4, Q4).
//!
//! The study loop diagnoses a dominant failure class (via `entelechy_failure`),
//! which names the complexity levels it makes eligible. This module asks the
//! [`ArchitectureExplorer`] whether that unlock is allowed (within the GoalSpec
//! ceiling, the design budget, and without adding authority), and if so emits the
//! typed [`EditOp`]s that realize the level — including the level-4/5 multi-agent
//! operators. Complexity is only proposed when a lower level's failure justifies
//! it (PRD principle 4).

use entelechy_design::{AgentSpec, EditOp};
use entelechy_ir::AuthorityEnvelope;

use crate::explorer::{level_description, ArchitectureExplorer, UnlockDecision};

/// A proposed next design mutation from the study loop (PRD 11.2/11.4).
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    /// The complexity level this proposal targets.
    pub level: u8,
    /// The typed edit operators to apply (smallest compiler-valid change first).
    pub ops: Vec<EditOp>,
    /// Human-readable rationale for the study narrative and audit log.
    pub rationale: String,
    /// Whether the unlock needs human approval before it may be applied: it is
    /// above the complexity ceiling or would add authority (PRD Q4).
    pub needs_approval: bool,
}

/// Propose the next design mutation given the diagnosed failure and the current
/// design complexity (PRD 11.3/11.4).
///
/// - `current_level`: the current design's complexity level (a single-agent
///   baseline is level 0).
/// - `class_id` / `eligible_levels`: the dominant failure class and the levels it
///   makes eligible (from `entelechy_failure`).
/// - `target_node`: the node the operators transform (e.g. the baseline agent).
/// - `parent_authority`: the authority in force at `target_node`, used to build a
///   validly-narrowed sub-agent for delegation (A2 / IR-I6).
/// - `ceiling`: the GoalSpec complexity ceiling; `budget_remaining`: whether the
///   design budget can fund exploration.
///
/// Returns `None` when no higher level is justified (the caller should stay at the
/// current level and apply a level-≤current repair, or stop).
pub fn propose(
    current_level: u8,
    class_id: &str,
    eligible_levels: &[u8],
    target_node: &str,
    parent_authority: &AuthorityEnvelope,
    ceiling: u8,
    budget_remaining: bool,
) -> Option<Proposal> {
    // Multi-agent operators narrow authority (delegation) or add none (parallel),
    // so the unlock never *adds* authority.
    let decision = ArchitectureExplorer::consider(
        current_level,
        eligible_levels,
        ceiling,
        budget_remaining,
        false,
    );
    let (level, needs_approval) = match decision {
        UnlockDecision::Automatic { level } => (level, false),
        UnlockDecision::NeedsApproval { level, .. } => (level, true),
        UnlockDecision::NoUnlock { .. } => return None,
    };
    let ops = ops_for_level(level, target_node, parent_authority)?;
    Some(Proposal {
        level,
        rationale: format!(
            "failure class '{class_id}' unlocks level {level} ({}); proposing {} operator(s)",
            level_description(level),
            ops.len()
        ),
        ops,
        needs_approval,
    })
}

/// The operator family that realizes a complexity level (PRD 11.4/11.5). Levels
/// with no design operator in this slice return `None`.
fn ops_for_level(
    level: u8,
    target: &str,
    parent_authority: &AuthorityEnvelope,
) -> Option<Vec<EditOp>> {
    let ops = match level {
        3 => vec![EditOp::AddVerify {
            id: format!("{target}_check"),
            checker: "supported".into(),
        }],
        4 => vec![EditOp::SplitParallel {
            node: target.to_string(),
            agents: vec![
                AgentSpec {
                    id: format!("{target}_a"),
                    prompt_template: "Sub-task A of the objective:\n{input}".to_string(),
                },
                AgentSpec {
                    id: format!("{target}_b"),
                    prompt_template: "Sub-task B of the objective:\n{input}".to_string(),
                },
            ],
        }],
        5 => vec![EditOp::AddDelegate {
            node: target.to_string(),
            wrapper_id: format!("{target}_agent"),
            // A reasoning-only sub-agent: no granted capabilities or scopes, but it
            // must carry forward every parent prohibition and stay within the
            // parent's financial ceiling to be a valid subset (A2 / IR-I6).
            authority: reasoning_only(parent_authority),
        }],
        _ => return None,
    };
    Some(ops)
}

/// The maximally-narrowed authority for a reasoning-only sub-agent that is always
/// a valid subset of `parent` (A2 / IR-I6): no grants or scopes, all of the
/// parent's prohibitions carried forward, and the parent's financial ceiling kept.
fn reasoning_only(parent: &AuthorityEnvelope) -> AuthorityEnvelope {
    AuthorityEnvelope {
        capabilities: Default::default(),
        forbidden_capabilities: parent.forbidden_capabilities.clone(),
        data_scopes: Default::default(),
        network_scopes: Default::default(),
        secret_scopes: Default::default(),
        financial_limit_minor: parent.financial_limit_minor,
        approval_required_for_writes: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent() -> AuthorityEnvelope {
        let mut a = AuthorityEnvelope::empty();
        a.capabilities.insert("draft_reply".into());
        a.forbidden_capabilities.insert("refund".into());
        a
    }

    #[test]
    fn verification_class_proposes_a_verify_step_at_level_3() {
        let p = propose(
            0,
            "reasoning.verification",
            &[3],
            "agent",
            &parent(),
            6,
            true,
        )
        .unwrap();
        assert_eq!(p.level, 3);
        assert!(!p.needs_approval);
        assert!(matches!(p.ops[0], EditOp::AddVerify { .. }));
    }

    #[test]
    fn decomposition_class_proposes_parallel_committee_at_level_4() {
        // reasoning.decomposition is eligible at [4, 5]; the smallest unlock is 4.
        let p = propose(
            0,
            "reasoning.decomposition",
            &[4, 5],
            "agent",
            &parent(),
            6,
            true,
        )
        .unwrap();
        assert_eq!(p.level, 4);
        match &p.ops[0] {
            EditOp::SplitParallel { node, agents } => {
                assert_eq!(node, "agent");
                assert_eq!(agents.len(), 2);
            }
            other => panic!("expected SplitParallel, got {other:?}"),
        }
    }

    #[test]
    fn coordination_class_proposes_a_valid_narrowed_delegate_at_level_5() {
        let par = parent();
        let p = propose(4, "coordination.handoff", &[5], "agent", &par, 6, true).unwrap();
        assert_eq!(p.level, 5);
        match &p.ops[0] {
            EditOp::AddDelegate { authority, .. } => {
                // The proposed sub-agent authority is a valid narrowing (A2/IR-I6):
                // it carries the parent's prohibition and grants nothing.
                assert!(authority.is_subset_of(&par));
                assert!(authority.forbidden_capabilities.contains("refund"));
                assert!(authority.capabilities.is_empty());
            }
            other => panic!("expected AddDelegate, got {other:?}"),
        }
    }

    #[test]
    fn no_unlock_when_no_higher_level_is_eligible() {
        // Already at level 5; classes only make lower levels eligible.
        assert!(propose(
            5,
            "reasoning.verification",
            &[3],
            "agent",
            &parent(),
            6,
            true
        )
        .is_none());
    }

    #[test]
    fn above_ceiling_needs_approval_not_silent_apply() {
        // Level 5 eligible but the ceiling is 4: escalation requires approval (Q4).
        let p = propose(3, "coordination.handoff", &[5], "agent", &parent(), 4, true).unwrap();
        assert_eq!(p.level, 5);
        assert!(p.needs_approval);
    }

    #[test]
    fn no_budget_means_no_proposal() {
        assert!(propose(
            0,
            "reasoning.decomposition",
            &[4],
            "agent",
            &parent(),
            6,
            false
        )
        .is_none());
    }
}
