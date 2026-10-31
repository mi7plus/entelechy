//! The unified Phase-0 study: the real design-repair loop, scored by *executing*
//! designs against the world (PRD §11.3, §21, §21.1).
//!
//! # Why this module exists
//!
//! Entelechy has two Phase-0 study drivers that, until now, each told only half
//! the story:
//!
//! - the **simulated** study (`entelechy study`) runs the *real* design-repair
//!   loop — diagnose ([`entelechy_failure::analyze`]) → propose the next operator
//!   ([`entelechy_search::propose`]) → apply it ([`entelechy_design::apply`]) →
//!   confirm on rolling validation ([`entelechy_search::rolling_validation_decision`])
//!   — but scores each candidate by reading its *structure* (does the IR contain a
//!   `Verify`? a `Par`?). That is circular: the thing being optimized is also the
//!   thing being scored.
//! - the **real-domain** study ([`crate::study::real_study`]) scores by *executing*
//!   each design against a resettable [`crate::world::HelpdeskWorld`] and reading
//!   world state — but its "repair" is a single hand-authored
//!   [`crate::study::repaired_design`], not a searched one.
//!
//! [`searched_study`] unifies them: it drives the **same** proposer loop the
//! simulated study uses, but swaps the scoring function for the **real executor**
//! ([`crate::exec::resolves`]). The result is not a cosmetic merge — it has teeth.
//! The suite carries three failure classes, and the loop discovers a repair for
//! each, each confirmed by *executing* the design, not by reading its structure:
//!
//! 1. the loop **discovers** (does not hand-author) the verification repair for
//!    the reply class (a `Verify` step at **level 3**), and the executor confirms
//!    it genuinely resolves the reply tickets the baseline failed;
//! 2. it then **discovers** the retrieval repair for the knowledge class (a
//!    `Retrieval` read at **level 2**, [`entelechy_design::EditOp::AddRetrieval`]) —
//!    *below* the level it just applied, because the complexity levels are a prior,
//!    not a strict ladder (PRD 11.4): each failure class escalates its own ladder,
//!    so the loop drops to level 2 for a retrieval miss even after accepting a
//!    level-3 fix. The executor confirms the retrieved fact resolves the knowledge
//!    tickets; a `Retrieval` adds no authority;
//! 3. when the loop proposes the level-4 parallel *committee* for the
//!    decomposition class, the executor **rejects** it — running it resolves
//!    nothing, because the committee's `Llm` sub-agents carry no part-resolving
//!    effect in this world — even though the structure-reading scorer would have
//!    *credited* that very committee (it "has a `Par`"); and
//! 4. the loop then escalates and **discovers** the decomposition repair too: a
//!    part-resolving `resolve_all` tool ([`entelechy_design::EditOp::AddTool`]),
//!    added at **level 5**, that the executor confirms resolves every part. It
//!    invokes a capability the agent already holds, so it widens no authority.
//!
//! So the repairs land at levels 3, 2, 5 — *not* in level order — which is the
//! non-ladder point made concrete. Point 3 is the honesty contrast: the same
//! search, scored two ways, and execution is the honest scorer — a structural
//! scorer silently credits a repair that does nothing when run; the executor does
//! not. Point 4 closes what used to be the gap: the decomposition class no longer
//! depends on the hand-authored `resolve_part` committee in
//! [`crate::study::real_study`] — the loop *discovers* its own part-resolving fix by
//! execution, exactly as it discovers the reply and retrieval fixes. The lessons
//! land in one run: more reasoning agents (the committee) resolve nothing here,
//! while a humble read, or an acting step the agent was already authorized for, does.
//!
//! The returned [`SearchedStudyReport`] carries one boolean per invariant plus the
//! discovered design, exactly like the operational drills and the other studies,
//! so the CLI and the TR-1 phase gate can both consume it.

use std::collections::BTreeMap;

use entelechy_design::{apply, EditOp};
use entelechy_eval::{
    CallerIdentity, ConstraintClass, Difficulty, EvalContract, GateResponse, HoldoutVault,
    NegativeGoal, Plane, Provenance, ReleaseRule, RiskClass, Split, SplitPolicy, Suite, Task,
};
use entelechy_failure::{analyze, FailureObservation, Symptom};
use entelechy_gateway::{MockModel, ModelGateway};
use entelechy_ir::{Node, NodeKind, Program};
use entelechy_search::{propose, rolling_validation_decision, Decision, Proposal};
use serde::{Deserialize, Serialize};

use crate::exec::resolves;
use crate::study::baseline_design;

/// The `Llm` node in the baseline the proposer transforms (the draft agent).
const DRAFT_NODE: &str = "draft";
/// The GoalSpec complexity ceiling: escalate automatically up to delegation
/// (level 5); anything above would need human approval (PRD 11.4, Q4).
const COMPLEXITY_CEILING: u8 = 5;
/// Candidate budget for the repair loop (PRD 21.1 Phase-0 default).
const CANDIDATE_BUDGET: u32 = 8;

/// The outcome of one executor-scored repair loop over the baseline.
#[derive(Clone, Debug)]
struct LoopOutcome {
    /// The best design the loop converged on (accepted repairs only).
    best: Program,
    /// Complexity levels of the accepted repairs, in order.
    accepted_levels: Vec<u8>,
    /// The parallel-committee candidate the loop proposed for the decomposition
    /// class, if it reached that level (whether or not it was accepted).
    committee_candidate: Option<Program>,
    /// Whether the executor-scored rolling validation rejected that committee.
    committee_rejected: bool,
    /// Number of candidates the loop evaluated (budget consumed).
    rounds: u32,
}

/// The unified-study report: one boolean per invariant plus the discovered design.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchedStudyReport {
    /// Whether the executor shows the baseline leaves every reply ticket
    /// unresolved (it drafts but never grounds).
    pub baseline_reply_unresolved: bool,
    /// Whether the executor shows the baseline leaves every decomposition ticket
    /// unresolved (it resolves only the first part).
    pub baseline_decomposition_unresolved: bool,
    /// Whether the executor shows the baseline leaves every knowledge ticket
    /// unresolved (it drafts an ungrounded guess without retrieving the fact).
    pub baseline_knowledge_unresolved: bool,
    /// Complexity levels of the repairs the loop accepted, in order of discovery
    /// (here `[3, 2, 5]` — deliberately not level-ordered: the levels are a prior,
    /// not a ladder, so a level-2 retrieval repair is found after a level-3 one).
    pub accepted_levels: Vec<u8>,
    /// Candidates the loop evaluated (budget consumed).
    pub rounds: u32,
    /// Whether the loop *discovered* a verification step not present in the
    /// baseline (the repair was searched, not hand-authored).
    pub discovered_verify: bool,
    /// Whether the executor confirms the discovered design resolves every reply
    /// ticket (tune + validation) the baseline failed.
    pub discovered_resolves_reply: bool,
    /// Whether the loop *discovered* a part-resolving tool (`resolve_all`) not
    /// present in the baseline — the decomposition repair, searched not hand-authored.
    pub discovered_part_resolver: bool,
    /// Whether the executor confirms the discovered design resolves every
    /// decomposition ticket (tune + validation) the baseline failed.
    pub discovered_resolves_decomposition: bool,
    /// Whether the loop *discovered* a `Retrieval` read not present in the baseline
    /// — the knowledge repair at level 2, searched not hand-authored.
    pub discovered_retrieval: bool,
    /// Whether the executor confirms the discovered design resolves every knowledge
    /// ticket (tune + validation) the baseline failed.
    pub discovered_resolves_knowledge: bool,
    /// Whether the loop proposed the level-4 parallel committee for the
    /// decomposition class.
    pub committee_proposed: bool,
    /// Whether the executor-scored loop rejected that committee (running it
    /// yielded no measured improvement).
    pub committee_rejected_by_execution: bool,
    /// Whether the structure-reading scorer would, by contrast, have *credited*
    /// that same committee for the decomposition class (it "has a `Par`").
    pub structure_would_credit_committee: bool,
    /// The honest-divergence invariant: execution rejected a committee that the
    /// structural scorer would have falsely credited.
    pub honest_divergence: bool,
    /// The holdout gate verdict over the discovered design (`pass`/`fail`/`refused`).
    pub holdout_result: String,
    /// Whether the discovered design passed the sealed holdout, scored by the
    /// executor (EV-14).
    pub holdout_passed: bool,
    /// The design the loop discovered (baseline plus the accepted repairs).
    pub discovered_design: Program,
    /// Overall: every invariant held.
    pub ok: bool,
}

/// Run the unified Phase-0 study (PRD §11.3, §21.1): drive the real design-repair
/// loop over the resettable helpdesk, scoring every candidate by **executing** it
/// against the world rather than by reading its structure.
///
/// See the [module docs](self) for what the invariants mean and why the
/// execution/structure divergence is the point.
pub fn searched_study() -> SearchedStudyReport {
    let gw = MockModel::new();
    let suite = searched_suite();
    let baseline = baseline_design();

    let tune: Vec<&Task> = suite.split(Split::Tune).collect();
    let val: Vec<&Task> = suite.split(Split::Validation).collect();

    // Sanity (executor-scored): the baseline leaves all three failure classes open.
    let baseline_reply_unresolved = tune
        .iter()
        .filter(|t| t.has_tag("needs_reply"))
        .all(|t| !resolves(&baseline, t, &gw));
    let baseline_decomposition_unresolved = tune
        .iter()
        .filter(|t| t.has_tag("needs_decomposition"))
        .all(|t| !resolves(&baseline, t, &gw));
    let baseline_knowledge_unresolved = tune
        .iter()
        .filter(|t| t.has_tag("needs_retrieval"))
        .all(|t| !resolves(&baseline, t, &gw));

    // Drive the proposer loop, scored by the real executor.
    let outcome = run_repair_loop(&baseline, &tune, &val, &gw);
    let discovered = outcome.best.clone();

    // The loop discovered a verification step the baseline did not have.
    let discovered_verify = has_verify(&discovered.root) && !has_verify(&baseline.root);

    // The executor confirms the discovered design resolves the reply class.
    let discovered_resolves_reply = tune
        .iter()
        .chain(val.iter())
        .filter(|t| t.has_tag("needs_reply"))
        .all(|t| resolves(&discovered, t, &gw));

    // The loop also discovered a part-resolving tool the baseline lacked, and the
    // executor confirms it resolves the decomposition class.
    let discovered_part_resolver =
        has_tool(&discovered.root, "resolve_all") && !has_tool(&baseline.root, "resolve_all");
    let discovered_resolves_decomposition = tune
        .iter()
        .chain(val.iter())
        .filter(|t| t.has_tag("needs_decomposition"))
        .all(|t| resolves(&discovered, t, &gw));

    // The loop also discovered a retrieval read the baseline lacked (at level 2,
    // below the verify it already applied), and the executor confirms it resolves
    // the knowledge class.
    let discovered_retrieval = has_retrieval(&discovered.root) && !has_retrieval(&baseline.root);
    let discovered_resolves_knowledge = tune
        .iter()
        .chain(val.iter())
        .filter(|t| t.has_tag("needs_retrieval"))
        .all(|t| resolves(&discovered, t, &gw));

    // The committee the loop proposed for decomposition: the executor rejected it,
    // yet the structure-reading scorer would have credited that very candidate.
    let committee_proposed = outcome.committee_candidate.is_some();
    let committee_rejected_by_execution = outcome.committee_rejected;
    let structure_would_credit_committee =
        outcome
            .committee_candidate
            .as_ref()
            .is_some_and(|candidate| {
                let decomposition: Vec<&Task> = tune
                    .iter()
                    .copied()
                    .filter(|t| t.has_tag("needs_decomposition"))
                    .collect();
                // The structural scorer credits every decomposition ticket (the design
                // "has a Par"), while the executor resolves none of them.
                decomposition
                    .iter()
                    .all(|t| structural_resolves(candidate, t))
                    && decomposition.iter().all(|t| !resolves(candidate, t, &gw))
            });
    let honest_divergence = committee_rejected_by_execution && structure_would_credit_committee;

    // Holdout gate (EV-14) over the discovered design, scored by the executor.
    let (holdout_result, holdout_passed) = run_holdout_gate(&baseline, &discovered, &suite);

    let ok = baseline_reply_unresolved
        && baseline_decomposition_unresolved
        && baseline_knowledge_unresolved
        && discovered_verify
        && discovered_resolves_reply
        && discovered_part_resolver
        && discovered_resolves_decomposition
        && discovered_retrieval
        && discovered_resolves_knowledge
        && honest_divergence
        && holdout_passed;

    SearchedStudyReport {
        baseline_reply_unresolved,
        baseline_decomposition_unresolved,
        baseline_knowledge_unresolved,
        accepted_levels: outcome.accepted_levels,
        rounds: outcome.rounds,
        discovered_verify,
        discovered_resolves_reply,
        discovered_part_resolver,
        discovered_resolves_decomposition,
        discovered_retrieval,
        discovered_resolves_knowledge,
        committee_proposed,
        committee_rejected_by_execution,
        structure_would_credit_committee,
        honest_divergence,
        holdout_result,
        holdout_passed,
        discovered_design: discovered,
        ok,
    }
}

/// The design the unified repair loop discovers (baseline plus the accepted
/// repairs), exposed on its own so other drivers can consume the *searched* repair
/// instead of a hand-authored one (PRD §11.3, §21.1).
///
/// This runs the exact loop [`searched_study`] runs — diagnose, propose, apply,
/// confirm on rolling validation, scored by [`resolves`] — over the three-class
/// `searched_suite` and [`baseline_design`], and returns its converged best design: the baseline
/// with the three repairs the executor confirmed (a `Verify` at level 3, a
/// `Retrieval` at level 2, and a `resolve_all` `Tool` at level 5). It is
/// deterministic, so it returns the same design every call.
///
/// [`crate::study::real_study`] consumes this to retire its hand-authored
/// [`crate::study::repaired_design`]: on the two-class [`crate::study::helpdesk_suite`]
/// the discovered design resolves every ticket identically to that committee (the
/// `Retrieval` is inert where no ticket needs it), so it is a drop-in replacement
/// discovered by execution rather than hand-authored.
pub fn discovered_repair() -> Program {
    let gw = MockModel::new();
    let suite = searched_suite();
    let baseline = baseline_design();
    let tune: Vec<&Task> = suite.split(Split::Tune).collect();
    let val: Vec<&Task> = suite.split(Split::Validation).collect();
    run_repair_loop(&baseline, &tune, &val, &gw).best
}

/// Drive the design-repair loop over the baseline, scoring each candidate by
/// executing it against the world (PRD 11.3). This is the exact proposer loop the
/// simulated study uses — diagnose, propose, apply, confirm on rolling validation —
/// with [`resolves`] as the scoring function in place of the structural scorer.
fn run_repair_loop(
    baseline: &Program,
    tune: &[&Task],
    val: &[&Task],
    gw: &dyn ModelGateway,
) -> LoopOutcome {
    let mut best = baseline.clone();
    // Each failure class escalates its *own* complexity ladder (PRD 11.4: the levels
    // are a prior, not a strict ladder). A single monotonic cursor would make a
    // level-2 retrieval repair unreachable once a level-3 verify repair had been
    // accepted — and worse, abandon the whole loop when the dominant class could
    // only propose a lower level. Tracking the level per class lets the loop
    // discover repairs in whatever level order the classes demand (here 3, 2, 5).
    let mut class_level: BTreeMap<String, u8> = BTreeMap::new();
    let mut rounds: u32 = 0;
    let mut accepted_levels = Vec::new();
    let mut committee_candidate: Option<Program> = None;
    let mut committee_rejected = false;

    while rounds < CANDIDATE_BUDGET {
        // Diagnose the current best design's executed failures on tune (PRD 10.2),
        // then pick the largest actionable cluster whose class can still escalate.
        // Classes that are exhausted (no higher eligible level) or gated behind
        // approval are skipped, so one stuck class never strands the others.
        let clusters = analyze(&observe_executed_failures(&best, tune, gw));
        let mut chosen: Option<(String, Proposal)> = None;
        for cluster in clusters.iter().filter(|c| !c.is_evaluation_failure) {
            let Some(top) = cluster.top() else { continue };
            let class_id = top.class_id.clone();
            let current = class_level.get(&class_id).copied().unwrap_or(0);
            match propose(
                current,
                &class_id,
                &top.eligible_levels,
                DRAFT_NODE,
                &best.authority,
                COMPLEXITY_CEILING,
                true,
            ) {
                // Above the ceiling or would add authority: cannot auto-apply, so
                // leave it for a human and try the next class (PRD 11.4, Q4).
                Some(p) if p.needs_approval => continue,
                Some(p) => {
                    chosen = Some((class_id, p));
                    break;
                }
                // This class cannot escalate further; try the next cluster.
                None => continue,
            }
        }
        let Some((class_id, proposal)) = chosen else {
            // No actionable class can escalate: converged.
            break;
        };
        rounds += 1;

        // Apply the proposed operators to the current best.
        let mut candidate = best.clone();
        let mut applied = true;
        for op in &proposal.ops {
            match apply(&candidate, op) {
                Ok(next) => candidate = next,
                Err(_) => {
                    applied = false;
                    break;
                }
            }
        }
        if !applied {
            class_level.insert(class_id, proposal.level);
            continue;
        }

        // Capture the parallel-committee candidate for the honesty contrast.
        let is_committee = proposal
            .ops
            .iter()
            .any(|op| matches!(op, EditOp::SplitParallel { .. }));
        if is_committee {
            committee_candidate = Some(candidate.clone());
        }

        // Score both designs by EXECUTION, then confirm on rolling validation.
        let base_tune = score_exec(&best, tune, gw);
        let cand_tune = score_exec(&candidate, tune, gw);
        let base_val = score_exec(&best, val, gw);
        let cand_val = score_exec(&candidate, val, gw);
        let decision = rolling_validation_decision(
            &base_tune,
            &cand_tune,
            &base_val,
            &cand_val,
            0xE0 + rounds as u64,
        );

        match decision {
            Decision::Accepted => {
                best = candidate;
                class_level.insert(class_id, proposal.level);
                accepted_levels.push(proposal.level);
            }
            Decision::Rejected | Decision::Inconclusive => {
                if is_committee {
                    committee_rejected = true;
                }
                // Escalate this class past the rejected level next round.
                class_level.insert(class_id, proposal.level);
            }
        }
    }

    LoopOutcome {
        best,
        accepted_levels,
        committee_candidate,
        committee_rejected,
        rounds,
    }
}

/// Turn a design's executed tune failures into failure observations (PRD 10.2).
/// Failure is decided by the executor ([`resolves`]); the symptom follows the
/// ticket class so the analyzer separates the three classes (an unresolved reply is
/// an unsupported claim → `reasoning.verification`; an unresolved multi-part ticket
/// is incomplete decomposition → `reasoning.decomposition`; an unresolved knowledge
/// ticket is a retrieval miss → `knowledge.retrieval_miss`).
fn observe_executed_failures(
    program: &Program,
    tasks: &[&Task],
    gw: &dyn ModelGateway,
) -> Vec<FailureObservation> {
    tasks
        .iter()
        .filter(|t| !resolves(program, t, gw))
        .map(|t| {
            let (signature, symptom) = if t.has_tag("needs_reply") {
                ("reply:unverified", Symptom::UnsupportedClaim)
            } else if t.has_tag("needs_retrieval") {
                ("knowledge:unretrieved", Symptom::RetrievalMiss)
            } else if t.has_tag("needs_decomposition") {
                ("task:fragmented", Symptom::IncompleteDecomposition)
            } else {
                ("unknown", Symptom::Unknown)
            };
            FailureObservation {
                task_id: t.id.clone(),
                signature: signature.into(),
                symptom,
                is_evaluation_failure: false,
            }
        })
        .collect()
}

/// Score a design on every task in a split by executing it against the world.
fn score_exec(program: &Program, tasks: &[&Task], gw: &dyn ModelGateway) -> Vec<bool> {
    tasks.iter().map(|t| resolves(program, t, gw)).collect()
}

/// The structure-reading scorer the real study was built to replace: it credits a
/// ticket by inspecting the design *structure*, not by running it. A reply ticket
/// is "resolved" once the design has any `Verify` node; a decomposition ticket once
/// it has any `Par` node. This is kept only to contrast it against the executor —
/// it is exactly the circular scorer [`searched_study`] exposes as dishonest.
fn structural_resolves(program: &Program, task: &Task) -> bool {
    if !program.authority.forbidden_capabilities.contains("refund") {
        return false;
    }
    if task.has_tag("needs_reply") {
        has_kind(&program.root, |k| matches!(k, NodeKind::Verify(_)))
    } else if task.has_tag("needs_decomposition") {
        has_kind(&program.root, |k| matches!(k, NodeKind::Par(_)))
    } else {
        true
    }
}

/// Whether any node in the design has a `Verify` kind.
fn has_verify(node: &Node) -> bool {
    has_kind(node, |k| matches!(k, NodeKind::Verify(_)))
}

/// Whether any node in the design is a `Tool` invoking the given capability.
fn has_tool(node: &Node, capability: &str) -> bool {
    has_kind(
        node,
        |k| matches!(k, NodeKind::Tool(t) if t.capability == capability),
    )
}

/// Whether any node in the design is a `Retrieval` read.
fn has_retrieval(node: &Node) -> bool {
    has_kind(node, |k| matches!(k, NodeKind::Retrieval(_)))
}

/// Whether any node in the design satisfies `pred` (depth-first).
fn has_kind(node: &Node, pred: impl Fn(&NodeKind) -> bool + Copy) -> bool {
    if pred(&node.kind) {
        return true;
    }
    match &node.kind {
        NodeKind::Seq(c) | NodeKind::Par(c) => c.iter().any(|n| has_kind(n, pred)),
        NodeKind::Map { body, .. }
        | NodeKind::Loop { body, .. }
        | NodeKind::Delegate { body, .. } => has_kind(body, pred),
        NodeKind::Branch { then, els, .. } => has_kind(then, pred) || has_kind(els, pred),
        _ => false,
    }
}

/// Gate the discovered design against the baseline on the sealed holdout using the
/// real executor as the scoring function (EV-14, PRD 9.6).
fn run_holdout_gate(baseline: &Program, discovered: &Program, suite: &Suite) -> (String, bool) {
    let contract = contract();
    let holdout: Vec<Task> = suite.split(Split::Holdout).cloned().collect();
    let mut vault = match HoldoutVault::seal(&contract, holdout) {
        Ok(v) => v,
        Err(_) => return ("refused".to_string(), false),
    };
    let assurance = CallerIdentity {
        id: "assure".into(),
        plane: Plane::Assurance,
    };
    let candidate_hash = entelechy_artifacts::ArtifactId::of(discovered)
        .map(|i| i.to_string())
        .unwrap_or_default();
    let base = baseline.clone();
    let cand = discovered.clone();
    // The holdout gate scores with its own deterministic mock (mock-r1), exactly
    // as the real-domain study does — the closures must own their gateway.
    let gw_base = MockModel::new();
    let gw_cand = MockModel::new();
    let resp = vault.gate_query(
        &assurance,
        &candidate_hash,
        &contract,
        &move |t| resolves(&base, t, &gw_base),
        &move |t| resolves(&cand, t, &gw_cand),
    );
    match resp {
        GateResponse::Pass { .. } => ("pass".to_string(), true),
        GateResponse::Fail { .. } => ("fail".to_string(), false),
        GateResponse::Refused { .. } => ("refused".to_string(), false),
    }
}

/// The suite for the unified study. Unlike the shared [`crate::study::helpdesk_suite`]
/// (two classes), this carries a third: knowledge tickets (`needs_retrieval`) that a
/// durable fact answers, so the loop has a level-2 retrieval repair to discover.
/// Keeping it separate leaves [`crate::study::real_study`]'s PV-2 drift narrative
/// untouched.
///
/// Tune counts order discovery by cluster size — reply 7 > knowledge 6 >
/// decomposition 5 — so the loop fixes verification (level 3) first, then retrieval
/// (level 2), then decomposition (committee rejected, resolve_all accepted at level
/// 5). Each class gets an equal validation share (4 each) so every repair clears the
/// rolling-validation bar independently (EV-13): a repair that newly resolves 4 of
/// 12 validation tasks has a paired-bootstrap lower bound above zero, while 3 of 12
/// would sit on the 2.5% boundary.
fn searched_suite() -> Suite {
    let mut tasks = Vec::new();
    let mut push = |split: Split, n: usize, tag: &str, base: &str, parts: Option<u64>| {
        for i in 0..n {
            let mut input = serde_json::json!({ "ticket": format!("{base}{i}") });
            if let Some(p) = parts {
                input["parts"] = serde_json::json!(p);
            }
            tasks.push(Task {
                id: format!("{base}{i}"),
                input,
                environment: "helpdesk".into(),
                checkers: vec!["helpdesk.resolved".into()],
                tags: vec![tag.to_string()],
                difficulty: Difficulty::Medium,
                provenance: Provenance::HumanSeed,
                split,
                source_task: None,
            });
        }
    };
    push(Split::Tune, 7, "needs_reply", "tune-reply", None);
    push(Split::Tune, 6, "needs_retrieval", "tune-know", None);
    push(Split::Tune, 5, "needs_decomposition", "tune-dec", Some(3));
    push(Split::Validation, 4, "needs_reply", "val-reply", None);
    push(Split::Validation, 4, "needs_retrieval", "val-know", None);
    push(
        Split::Validation,
        4,
        "needs_decomposition",
        "val-dec",
        Some(3),
    );
    push(Split::Holdout, 7, "needs_reply", "hold-reply", None);
    push(Split::Holdout, 6, "needs_retrieval", "hold-know", None);
    push(
        Split::Holdout,
        5,
        "needs_decomposition",
        "hold-dec",
        Some(3),
    );
    Suite::new(tasks)
}

/// The eval contract for the unified study (mirrors [`crate::study`], PRD 9, 21.1).
/// The split sizes track [`searched_suite`] (tune 18, validation 12, holdout 18).
fn contract() -> EvalContract {
    EvalContract {
        version: 1,
        criteria: vec!["resolves_tier1_ticket".into()],
        negative_goals: vec![NegativeGoal {
            name: "no_refund".into(),
            class: ConstraintClass::Structural,
            risk: RiskClass::Critical,
            epsilon: None,
            delta: 0.05,
        }],
        splits: SplitPolicy {
            tune: 18,
            validation: 12,
            holdout: 18,
        },
        release: ReleaseRule {
            primary_metric: "task_success".into(),
            target_improvement_pp: 20.0,
            holdout_query_budget: 5,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searched_study_is_green() {
        let r = searched_study();
        assert!(
            r.baseline_reply_unresolved,
            "baseline leaves reply tickets unresolved"
        );
        assert!(
            r.baseline_decomposition_unresolved,
            "baseline leaves decomposition tickets unresolved"
        );
        assert!(
            r.baseline_knowledge_unresolved,
            "baseline leaves knowledge tickets unresolved"
        );
        assert!(
            r.discovered_verify,
            "the loop discovered a verification step not in the baseline"
        );
        assert!(
            r.discovered_resolves_reply,
            "the executor confirms the discovered design resolves the reply class"
        );
        assert!(
            r.discovered_retrieval,
            "the loop discovered a retrieval read not in the baseline"
        );
        assert!(
            r.discovered_resolves_knowledge,
            "the executor confirms the discovered design resolves the knowledge class"
        );
        assert!(
            r.discovered_part_resolver,
            "the loop discovered a part-resolving tool not in the baseline"
        );
        assert!(
            r.discovered_resolves_decomposition,
            "the executor confirms the discovered design resolves the decomposition class"
        );
        assert!(
            r.committee_proposed,
            "the loop proposed the parallel committee for decomposition"
        );
        assert!(
            r.committee_rejected_by_execution,
            "the executor rejected the committee (it resolves nothing when run)"
        );
        assert!(
            r.structure_would_credit_committee,
            "the structural scorer would have credited that committee"
        );
        assert!(
            r.honest_divergence,
            "execution rejected a repair the structural scorer would have credited"
        );
        assert!(
            r.holdout_passed,
            "the discovered design passed the holdout gate: {}",
            r.holdout_result
        );
        assert!(r.ok, "every invariant held");
    }

    #[test]
    fn the_discovered_design_adds_a_verify_a_retrieval_and_a_resolver() {
        let r = searched_study();
        // The loop accepted three repairs by execution, in discovery order: the
        // verification step (level 3, reply class), then — dropping *below* it — the
        // retrieval read (level 2, knowledge class), then the part-resolving tool
        // (level 5, decomposition class). The level-4 reasoning committee was proposed
        // between the last two and rejected — it never stuck. So the accepted levels
        // are exactly [3, 2, 5]: deliberately not level-ordered, which is the point.
        assert_eq!(
            r.accepted_levels,
            vec![3, 2, 5],
            "verify (3), then retrieval (2) below it, then the part-resolver (5); the committee did not stick"
        );
        assert!(has_verify(&r.discovered_design.root));
        assert!(has_retrieval(&r.discovered_design.root));
        assert!(has_tool(&r.discovered_design.root, "resolve_all"));
    }

    #[test]
    fn the_committee_the_executor_rejects_is_one_structure_scoring_accepts() {
        // The honest-divergence invariant, stated directly: the searched loop,
        // scored by execution, rejects the parallel committee; the same candidate
        // is credited by the structure-reading scorer. Same search, two scorers.
        let r = searched_study();
        assert!(r.committee_rejected_by_execution);
        assert!(r.structure_would_credit_committee);
    }

    #[test]
    fn searched_study_is_deterministic() {
        assert_eq!(searched_study(), searched_study());
    }

    #[test]
    fn discovered_repair_is_the_design_the_study_converges_on() {
        // The exposed repair is exactly the loop's converged best — the same design
        // `searched_study` reports — so a consumer that retires a hand-authored repair
        // for the searched one gets what the study verified.
        let repair = discovered_repair();
        assert_eq!(repair, searched_study().discovered_design);
        // And it carries the three repairs the executor confirmed.
        assert!(has_verify(&repair.root));
        assert!(has_retrieval(&repair.root));
        assert!(has_tool(&repair.root, "resolve_all"));
    }
}
