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
//! ([`crate::exec::resolves`]). The result is not a cosmetic merge — it has teeth:
//!
//! 1. the loop **discovers** (does not hand-author) the verification repair for
//!    the reply class, and the executor confirms it genuinely resolves the reply
//!    tickets the baseline failed; and
//! 2. when the loop proposes the level-4 parallel *committee* for the
//!    decomposition class, the executor **rejects** it — running it resolves
//!    nothing, because the committee's `Llm` sub-agents carry no part-resolving
//!    effect in this world — even though the structure-reading scorer would have
//!    *credited* that very committee (it "has a `Par`").
//!
//! That divergence is the point: the same search, scored two ways, and execution
//! is the honest scorer. A structural scorer silently credits a repair that does
//! nothing when run; the executor does not. The decomposition class therefore
//! remains resolved only by the hand-authored `resolve_part` committee in
//! [`crate::study::real_study`] — closing that gap needs a part-resolving edit
//! operator in the proposer's vocabulary, which is a named next step, not a gap
//! this module papers over.
//!
//! The returned [`SearchedStudyReport`] carries one boolean per invariant plus the
//! discovered design, exactly like the operational drills and the other studies,
//! so the CLI and the TR-1 phase gate can both consume it.

use entelechy_design::{apply, EditOp};
use entelechy_eval::{
    CallerIdentity, ConstraintClass, EvalContract, GateResponse, HoldoutVault, NegativeGoal, Plane,
    ReleaseRule, RiskClass, Split, SplitPolicy, Suite, Task,
};
use entelechy_failure::{analyze, FailureObservation, Symptom};
use entelechy_gateway::{MockModel, ModelGateway};
use entelechy_ir::{Node, NodeKind, Program};
use entelechy_search::{propose, rolling_validation_decision, Decision};
use serde::{Deserialize, Serialize};

use crate::exec::resolves;
use crate::study::{baseline_design, helpdesk_suite};

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
    /// Complexity levels of the repairs the loop accepted, in order.
    pub accepted_levels: Vec<u8>,
    /// Candidates the loop evaluated (budget consumed).
    pub rounds: u32,
    /// Whether the loop *discovered* a verification step not present in the
    /// baseline (the repair was searched, not hand-authored).
    pub discovered_verify: bool,
    /// Whether the executor confirms the discovered design resolves every reply
    /// ticket (tune + validation) the baseline failed.
    pub discovered_resolves_reply: bool,
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
    let suite = helpdesk_suite();
    let baseline = baseline_design();

    let tune: Vec<&Task> = suite.split(Split::Tune).collect();
    let val: Vec<&Task> = suite.split(Split::Validation).collect();

    // Sanity (executor-scored): the baseline leaves both failure classes open.
    let baseline_reply_unresolved = tune
        .iter()
        .filter(|t| t.has_tag("needs_reply"))
        .all(|t| !resolves(&baseline, t, &gw));
    let baseline_decomposition_unresolved = tune
        .iter()
        .filter(|t| t.has_tag("needs_decomposition"))
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
        && discovered_verify
        && discovered_resolves_reply
        && honest_divergence
        && holdout_passed;

    SearchedStudyReport {
        baseline_reply_unresolved,
        baseline_decomposition_unresolved,
        accepted_levels: outcome.accepted_levels,
        rounds: outcome.rounds,
        discovered_verify,
        discovered_resolves_reply,
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
    let mut level: u8 = 0;
    let mut rounds: u32 = 0;
    let mut accepted_levels = Vec::new();
    let mut committee_candidate: Option<Program> = None;
    let mut committee_rejected = false;

    while rounds < CANDIDATE_BUDGET {
        // Diagnose the current best design's executed failures on tune (PRD 10.2).
        let clusters = analyze(&observe_executed_failures(&best, tune, gw));
        let Some((class_id, eligible)) = clusters
            .iter()
            .find(|c| !c.is_evaluation_failure)
            .and_then(|c| {
                c.top()
                    .map(|t| (t.class_id.clone(), t.eligible_levels.clone()))
            })
        else {
            // Converged: no actionable failure remains in the best design.
            break;
        };

        // Propose the next mutation, escalating from the current level (PRD 11.4).
        let proposal = match propose(
            level,
            &class_id,
            &eligible,
            DRAFT_NODE,
            &best.authority,
            COMPLEXITY_CEILING,
            true,
        ) {
            // An above-ceiling unlock needs human approval; stop rather than apply.
            Some(p) if p.needs_approval => break,
            Some(p) => p,
            // No higher level is justified: converged.
            None => break,
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
            level = proposal.level;
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
                level = proposal.level;
                accepted_levels.push(proposal.level);
            }
            Decision::Rejected | Decision::Inconclusive => {
                if is_committee {
                    committee_rejected = true;
                }
                // Escalate past this level next round.
                level = proposal.level;
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
/// ticket class so the analyzer separates the two classes (an unresolved reply is
/// an unsupported claim → `reasoning.verification`; an unresolved multi-part ticket
/// is incomplete decomposition → `reasoning.decomposition`).
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

/// The eval contract for the unified study (mirrors [`crate::study`], PRD 9, 21.1).
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
            tune: 12,
            validation: 6,
            holdout: 12,
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
            r.discovered_verify,
            "the loop discovered a verification step not in the baseline"
        );
        assert!(
            r.discovered_resolves_reply,
            "the executor confirms the discovered design resolves the reply class"
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
    fn the_discovered_design_is_the_baseline_plus_a_verify() {
        let r = searched_study();
        // The loop accepted exactly the verification repair (level 3); the
        // committee (4) and delegate (5) were tried and rejected by the executor.
        assert_eq!(r.accepted_levels, vec![3], "only the verify repair stuck");
        assert!(has_verify(&r.discovered_design.root));
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
}
