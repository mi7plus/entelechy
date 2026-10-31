//! The Design-IR interpreter over the helpdesk world (PRD §21, 7.1).
//!
//! [`execute`] walks a [`Program`] and applies each node's *effect* to a fresh
//! [`HelpdeskWorld`]: an `Llm` node drafts an ungrounded reply (through a real
//! [`ModelGateway`] call, so the provider identity is captured — PV-1), a `Verify`
//! node grounds it, a `Retrieval` node reads durable knowledge into the world, a
//! `resolve_part` tool resolves one part, a `Par` committee of workers resolves
//! every part, and a forbidden `refund` tool is refused. Success is then read by
//! the [`HelpdeskWorld::resolved`] checker — from world state, not from the program.

use entelechy_eval::Task;
use entelechy_gateway::{ModelGateway, ModelRequest, ProviderIdentity};
use entelechy_ir::{AuthorityEnvelope, Node, NodeKind, Program};
use serde::{Deserialize, Serialize};

use crate::world::HelpdeskWorld;

/// The result of executing a design against the world for one task: the final
/// world (for the checker) and every provider identity captured along the way
/// (for fingerprinting — PV-1/PV-2).
#[derive(Clone, Debug)]
pub struct Execution {
    /// The world after the design has run.
    pub world: HelpdeskWorld,
    /// Provider identities recorded, one per model call (PV-1).
    pub identities: Vec<ProviderIdentity>,
}

/// Execute `program` against a fresh world for `task`, using `gateway` for model
/// calls. Returns the final world and the captured provider identities.
pub fn execute(program: &Program, task: &Task, gateway: &dyn ModelGateway) -> Execution {
    let mut world = HelpdeskWorld::for_task(task);
    let mut identities = Vec::new();
    apply(
        &program.root,
        &program.authority,
        task,
        gateway,
        &mut world,
        &mut identities,
    );
    Execution { world, identities }
}

/// Whether a design resolves a task on the real domain (the scoring function used
/// by the comparator and the holdout gate).
pub fn resolves(program: &Program, task: &Task, gateway: &dyn ModelGateway) -> bool {
    execute(program, task, gateway).world.resolved()
}

/// Walk one node, applying its effect to the world.
fn apply(
    node: &Node,
    authority: &AuthorityEnvelope,
    task: &Task,
    gateway: &dyn ModelGateway,
    world: &mut HelpdeskWorld,
    identities: &mut Vec<ProviderIdentity>,
) {
    match &node.kind {
        NodeKind::Llm(llm) => {
            let req = ModelRequest {
                model: llm.model.clone(),
                prompt: llm
                    .prompt_template
                    .replace("{input}", &task.input.to_string()),
                temperature: llm.temperature,
            };
            // A model failure leaves the reply undrafted (fail-closed for
            // resolution — an unresolved ticket, never a false success).
            if let Ok(resp) = gateway.infer(&req) {
                let revision = resp.identity.revision.clone().unwrap_or_default();
                world.draft_reply(resp.text, revision);
                identities.push(resp.identity);
            }
        }
        NodeKind::Verify(_) => {
            world.verify_reply();
        }
        // Retrieval reads durable knowledge into the world (PRD 12, 11.4 level 2).
        // It is a read, not a capability invocation — it adds no authority — so it
        // is applied unconditionally; the world decides whether the configured tier
        // and budget actually surface the needed fact.
        NodeKind::Retrieval(r) => {
            world.retrieve(r.tier, r.top_k);
        }
        NodeKind::Tool(tool) => {
            apply_tool(&tool.capability, &tool.args, authority, world);
        }
        // Composition: thread the world through children in order. A `Par`
        // committee is a set of workers run against the same world.
        NodeKind::Seq(children) | NodeKind::Par(children) => {
            for child in children {
                apply(child, authority, task, gateway, world, identities);
            }
        }
        // Scoped sub-agent: the body runs under narrowed authority (IR-I6). The
        // child authority is a subset, so forbidden capabilities stay forbidden.
        NodeKind::Delegate {
            authority: sub,
            body,
        } => {
            apply(body, sub, task, gateway, world, identities);
        }
        // Nodes without a helpdesk effect in this Phase-0 slice are inert.
        _ => {}
    }
}

/// Apply a tool invocation, honoring the authority envelope (IR-I2). A capability
/// that is forbidden, or not granted, is refused; `refund` in particular is
/// attempted against the world (which always refuses it) so the refusal is
/// observable, and never issued.
fn apply_tool(
    capability: &str,
    args: &serde_json::Value,
    authority: &AuthorityEnvelope,
    world: &mut HelpdeskWorld,
) {
    if authority.forbidden_capabilities.contains(capability) {
        if capability == "refund" {
            world.attempt_refund();
        }
        return;
    }
    if !authority.capabilities.contains(capability) {
        // Ungranted capability: refused (fail-closed), with refund still observed.
        if capability == "refund" {
            world.attempt_refund();
        }
        return;
    }
    match capability {
        "resolve_part" => {
            let index = args.get("part").and_then(|p| p.as_u64()).unwrap_or(0) as usize;
            world.resolve_part(index);
        }
        "resolve_all" => world.resolve_all_parts(),
        "refund" => {
            world.attempt_refund();
        }
        _ => {}
    }
}

/// One task's outcome under both designs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDelta {
    /// Task id.
    pub task_id: String,
    /// Whether the baseline resolved it.
    pub baseline: bool,
    /// Whether the candidate resolved it.
    pub candidate: bool,
}

/// The comparator: baseline vs candidate over a set of tasks on the real domain
/// (PRD §21.1 — a design change is justified by a measured improvement, here
/// resolved-ticket counts from programmatic state checkers).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparisonReport {
    /// Tasks the baseline resolved.
    pub baseline_resolved: usize,
    /// Tasks the candidate resolved.
    pub candidate_resolved: usize,
    /// Total tasks compared.
    pub total: usize,
    /// Per-task outcomes.
    pub deltas: Vec<TaskDelta>,
}

impl ComparisonReport {
    /// Tasks the candidate newly resolves (candidate yes, baseline no).
    pub fn improved(&self) -> usize {
        self.deltas
            .iter()
            .filter(|d| d.candidate && !d.baseline)
            .count()
    }

    /// Tasks the candidate regresses (baseline yes, candidate no).
    pub fn regressed(&self) -> usize {
        self.deltas
            .iter()
            .filter(|d| d.baseline && !d.candidate)
            .count()
    }

    /// Whether the candidate strictly improves on the baseline with no regression.
    pub fn candidate_dominates(&self) -> bool {
        self.improved() > 0 && self.regressed() == 0
    }
}

/// Run both designs over every task and build the comparison.
pub fn compare(
    baseline: &Program,
    candidate: &Program,
    tasks: &[&Task],
    gateway: &dyn ModelGateway,
) -> ComparisonReport {
    let mut deltas = Vec::with_capacity(tasks.len());
    let (mut baseline_resolved, mut candidate_resolved) = (0, 0);
    for task in tasks {
        let b = resolves(baseline, task, gateway);
        let c = resolves(candidate, task, gateway);
        baseline_resolved += usize::from(b);
        candidate_resolved += usize::from(c);
        deltas.push(TaskDelta {
            task_id: task.id.clone(),
            baseline: b,
            candidate: c,
        });
    }
    ComparisonReport {
        baseline_resolved,
        candidate_resolved,
        total: tasks.len(),
        deltas,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::study::{baseline_design, repaired_design};
    use crate::world::TicketKind;
    use entelechy_eval::{Difficulty, Provenance, Split};
    use entelechy_gateway::MockModel;

    fn task(id: &str, tag: &str, parts: Option<u64>) -> Task {
        let input = match parts {
            Some(p) => serde_json::json!({ "ticket": id, "parts": p }),
            None => serde_json::json!({ "ticket": id }),
        };
        Task {
            id: id.into(),
            input,
            environment: "helpdesk".into(),
            checkers: vec!["resolved".into()],
            tags: vec![tag.into()],
            difficulty: Difficulty::Medium,
            provenance: Provenance::HumanSeed,
            split: Split::Tune,
            source_task: None,
        }
    }

    #[test]
    fn baseline_fails_what_the_repaired_design_resolves() {
        let gw = MockModel::new();
        let reply = task("r", "needs_reply", None);
        let dec = task("d", "needs_decomposition", Some(3));

        // Baseline: drafts but never grounds, resolves only the first part.
        assert!(!resolves(&baseline_design(), &reply, &gw));
        assert!(!resolves(&baseline_design(), &dec, &gw));

        // Repaired: verification grounds the reply; the committee resolves all parts.
        assert!(resolves(&repaired_design(), &reply, &gw));
        assert!(resolves(&repaired_design(), &dec, &gw));
    }

    #[test]
    fn a_knowledge_ticket_needs_a_retrieval_step() {
        use entelechy_ir::{
            AuthorityEnvelope, LlmNode, MemoryTier, Node, NodeKind, Program, RetrievalNode,
        };
        let gw = MockModel::new();
        let know = task("k", "needs_retrieval", None);

        // Baseline drafts a reply but never retrieves the needed fact -> unresolved.
        assert!(!resolves(&baseline_design(), &know, &gw));

        // Prepending a semantic retrieval before the draft surfaces the fact, and
        // the knowledge ticket resolves. The retrieval adds no authority.
        let mut auth = AuthorityEnvelope::empty();
        auth.capabilities.insert("draft_reply".into());
        auth.forbidden_capabilities.insert("refund".into());
        let retrieving = Program::new(
            auth,
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "fetch",
                        NodeKind::Retrieval(RetrievalNode {
                            query_template: "{input}".into(),
                            tier: MemoryTier::Semantic,
                            top_k: 4,
                        }),
                    ),
                    Node::new(
                        "draft",
                        NodeKind::Llm(LlmNode {
                            model: "mock-small".into(),
                            prompt_template: "resolve ticket: {input}".into(),
                            temperature: 0.0,
                        }),
                    ),
                ]),
            ),
        );
        assert!(resolves(&retrieving, &know, &gw));
    }

    #[test]
    fn execution_captures_a_provider_identity_per_model_call() {
        let gw = MockModel::new();
        let reply = task("r", "needs_reply", None);
        let exec = execute(&repaired_design(), &reply, &gw);
        assert_eq!(exec.identities.len(), 1, "one Llm node, one identity");
        assert_eq!(exec.identities[0].provider, "mock");
    }

    #[test]
    fn comparator_counts_improvements_without_regression() {
        let gw = MockModel::new();
        let reply = task("r", "needs_reply", None);
        let dec = task("d", "needs_decomposition", Some(3));
        let tasks = [&reply, &dec];
        let report = compare(&baseline_design(), &repaired_design(), &tasks, &gw);
        assert_eq!(report.baseline_resolved, 0);
        assert_eq!(report.candidate_resolved, 2);
        assert_eq!(report.improved(), 2);
        assert_eq!(report.regressed(), 0);
        assert!(report.candidate_dominates());
    }

    #[test]
    fn forbidden_refund_is_refused_and_never_issued() {
        // A design that tries to refund: the authority forbids it, the world refuses
        // it, and the negative goal holds.
        let gw = MockModel::new();
        let reply = task("r", "needs_reply", None);
        let design = crate::study::refund_attempting_design();
        let exec = execute(&design, &reply, &gw);
        assert!(
            exec.world.refund_attempts() >= 1,
            "the refund was attempted"
        );
        assert!(exec.world.negative_goal_held(), "but never issued");
    }

    #[test]
    fn a_world_resolved_by_committee_matches_direct_resolution() {
        // Sanity: executing the repaired design on a 3-part ticket resolves exactly
        // the parts a direct committee call would.
        let gw = MockModel::new();
        let dec = task("d", "needs_decomposition", Some(3));
        let exec = execute(&repaired_design(), &dec, &gw);
        assert_eq!(exec.world.resolved_part_count(), 3);
        assert_eq!(*exec.world.kind(), TicketKind::Decomposition { parts: 3 });
    }
}
