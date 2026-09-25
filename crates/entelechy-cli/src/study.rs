//! A runnable Phase 0 study driver demo (PRD 21).
//!
//! This wires the whole optimization loop together on the simulated support-triage
//! benchmark (Q1): synthesize a single-agent baseline, pre-register a signed
//! StudyPlan, run one repair hypothesis with candidate accounting and
//! rolling-validation confirmation (EV-13), then gate the accepted candidate on a
//! sealed holdout (EV-14).
//!
//! Honesty note: the mock model carries no semantic quality signal, so success is
//! decided by a *declared simulated environment* — exactly the Phase 0 shape
//! ("simulated helpdesk with programmatic state checkers", PRD 21, Q1). The
//! simulator models the failure class from PRD Appendix C: without a verification
//! step, tickets that need a grounded reply fail; adding the `reply_check`
//! verification step (a real typed patch, PRD 11.5) fixes that class.
//!
//! Real-model path: built with `--features openai` and with `ENTELECHY_BASE_URL`
//! / `ENTELECHY_MODEL` (optional `ENTELECHY_API_KEY`) set, the study also drives a
//! live self-hosted, OpenAI-compatible model for governed task synthesis (EV-3,
//! Q2, 9.5) via [`maybe_synthesize_tasks`]. Without those, that step is a no-op and
//! the default/CI run is unchanged.

use entelechy_bench::{BenchmarkManifest, StoppingRule, StudyPlan};
use entelechy_design::{apply, synthesize_single_agent, DesignHypothesis, HypothesisResult};
use entelechy_eval::{
    CallerIdentity, ConstraintClass, Difficulty, EvalContract, GateResponse, HoldoutVault,
    NegativeGoal, Plane, Provenance, ReleaseRule, RiskClass, Split, SplitPolicy, Suite, Task,
};
use entelechy_failure::{FailureObservation, Symptom};
use entelechy_ir::{AuthorityEnvelope, Node, NodeKind, Program};
use entelechy_search::{propose, rolling_validation_decision, Candidate, Decision, Study};

/// The baseline single-agent node id (`synthesize_single_agent` names it `agent`).
const AGENT_NODE: &str = "agent";
/// The GoalSpec complexity ceiling for this demo: allow escalation up to delegation
/// (level 5) automatically; anything above needs approval (PRD 11.4, Q4).
const COMPLEXITY_CEILING: u8 = 5;

/// Run the Phase 0 study demo, printing a narrative of each governed step. When
/// `out` is set, the final best design IR and a study report are written there
/// (durable, content-addressed artifacts — PRD principle 6).
pub fn run(out: Option<&str>) -> anyhow::Result<()> {
    // --- 1. Baseline design (PRD 11.1: mandatory single-agent baseline) ---
    let mut authority = AuthorityEnvelope::empty();
    authority.capabilities.insert("draft_reply".into());
    authority.forbidden_capabilities.insert("refund".into()); // structural negative goal
    let baseline = synthesize_single_agent(authority, "mock-small", "resolve ticket: {input}");
    let baseline_id = entelechy_artifacts::ArtifactId::of(&baseline)?;
    println!("1. Baseline synthesized (single agent) — {baseline_id}");

    // --- 2. EvalContract + benchmark + signed StudyPlan (PRD 21.1) ---
    let suite = helpdesk_suite();
    let contract = contract();
    let manifest = BenchmarkManifest::from_suite("helpdesk-triage", &suite);
    // Contamination: no near-duplicate may cross tune -> validation/holdout (9.5).
    let findings = manifest.cross_split_scan(&suite);
    println!(
        "2. EvalContract v{}, benchmark manifest {} ({} holdout tasks sealed), cross-split findings: {}",
        contract.version,
        manifest.id(),
        manifest.holdout_hashes().len(),
        findings.len()
    );

    // --- 2a. Optional: real task synthesis via a live model (EV-3) ---
    // No-op unless built with `--features openai` and ENTELECHY_BASE_URL/MODEL set.
    maybe_synthesize_tasks(&suite);

    let plan = StudyPlan {
        revision: 1,
        manifest_id: manifest.id().to_string(),
        baseline_id: baseline_id.to_string(),
        primary_metric: contract.release.primary_metric.clone(),
        splits: contract.splits.clone(),
        mde_validation_pp: entelechy_eval::min_detectable_effect_pp(
            suite.split_len(Split::Validation),
        ),
        mde_holdout_pp: entelechy_eval::min_detectable_effect_pp(suite.split_len(Split::Holdout)),
        stopping: StoppingRule::Phase0Default {
            candidate_budget: 8,
        },
        analysis: "paired bootstrap, 95% interval".into(),
        frozen_models: vec!["mock-small@mock-r1".into()],
        signed_by: "owner@example".into(),
    };
    println!(
        "   StudyPlan signed by {} — commitment {} ({}).",
        plan.signed_by,
        plan.id(),
        Study::describe_rule(&plan)
    );

    // --- 3. Structural safety of the baseline (PRD 5.6 / 7.4) ---
    let catalog = crate::demo::demo_catalog();
    let base_violations = entelechy_ir::validate(&baseline, &catalog);
    println!(
        "3. Structural check: {} violation(s) — the refund negative goal is enforced by absent authority.",
        base_violations.len()
    );

    // --- 4. Iterative repair loop: propose, evaluate, escalate (PRD 11.3, 21.1) ---
    let mut study = Study::from_plan(&plan);
    let tune: Vec<&Task> = suite.split(Split::Tune).collect();
    let val: Vec<&Task> = suite.split(Split::Validation).collect();

    println!(
        "4. Baseline success — tune {:.0}%, validation {:.0}%.",
        pct(&score_all(&baseline, &tune)),
        pct(&score_all(&baseline, &val))
    );

    // Keep the best confirmed design; each round, re-diagnose its remaining
    // failures and escalate the complexity level only when evidence and the
    // Architecture Explorer justify it (single agent → verification → parallel
    // committee → multi-agent delegation). Every evaluated candidate consumes
    // budget, whether or not it is retained (EV-13, 21.1).
    let mut best = baseline.clone();
    let mut level: u8 = 0;
    let mut round: u64 = 0;
    let mut hyps: Vec<DesignHypothesis> = Vec::new();
    loop {
        if study.remaining() == 0 {
            println!("   Budget exhausted; stopping.");
            break;
        }
        // Diagnose the current best design's tune failures (PRD 10.2).
        let clusters = entelechy_failure::analyze(&observe_failures(&best, &tune));
        let Some((cluster, cls)) = clusters
            .iter()
            .find(|c| !c.is_evaluation_failure)
            .and_then(|c| c.top().map(|t| (c, t)))
        else {
            println!("   Converged: no actionable failures remain in the best design.");
            break;
        };
        println!(
            "   Diagnosis: cluster '{}' — {} ({} tasks, eligible levels {:?}).",
            cluster.id,
            cls.class_id,
            cluster.members.len(),
            cls.eligible_levels
        );

        // Propose the next mutation, escalating from the current level (PRD 11.4).
        let proposal = match propose(
            level,
            &cls.class_id,
            &cls.eligible_levels,
            AGENT_NODE,
            &best.authority,
            COMPLEXITY_CEILING,
            true,
        ) {
            Some(p) if p.needs_approval => {
                println!(
                    "   → level {} needs human approval (Q4); stopping.",
                    p.level
                );
                break;
            }
            Some(p) => p,
            None => {
                println!("   → no further complexity unlock is justified; stopping.");
                break;
            }
        };

        // Account every evaluated candidate against the budget (PRD 21.1).
        if study.account_candidate().is_err() {
            println!("   Budget exhausted; stopping.");
            break;
        }
        round += 1;

        // Apply the proposed operators to the current best.
        let mut candidate = best.clone();
        for op in &proposal.ops {
            candidate = apply(&candidate, op)?;
        }
        let base_tune = score_all(&best, &tune);
        let cand_tune = score_all(&candidate, &tune);
        let base_val = score_all(&best, &val);
        let cand_val = score_all(&candidate, &val);
        let decision = rolling_validation_decision(
            &base_tune,
            &cand_tune,
            &base_val,
            &cand_val,
            20_250_920 + round,
        );

        let mut hyp = DesignHypothesis {
            id: format!("H-{round}"),
            evidence: format!("cluster '{}': {} tasks", cluster.id, cluster.members.len()),
            failure_class: cls.class_id.clone(),
            suspected_cause: format!(
                "class '{}' unresolved at complexity level {level}",
                cls.class_id
            ),
            cause_confidence: cls.probability,
            patch: proposal.ops.clone(),
            expected_effect: "raise task success on the diagnosed failure class".into(),
            expected_tradeoff: "higher cost/latency and structural complexity".into(),
            experiment: "paired eval vs the current best on tune, confirm on validation".into(),
            result: HypothesisResult::Pending,
        };
        println!(
            "   H-{round}: {} — tune {:.0}%→{:.0}%, val {:.0}%→{:.0}% → {decision:?}.",
            proposal.rationale,
            pct(&base_tune),
            pct(&cand_tune),
            pct(&base_val),
            pct(&cand_val)
        );

        match decision {
            Decision::Accepted => {
                study.accept(Candidate {
                    design_hash: entelechy_artifacts::ArtifactId::of(&candidate)?.to_string(),
                    validation_success: frac(&cand_val),
                });
                best = candidate;
                level = proposal.level;
                hyp.resolve(HypothesisResult::Accepted);
                println!(
                    "      Rolling validation → Accepted; best is now at complexity level {level}."
                );
            }
            Decision::Rejected => {
                level = proposal.level; // escalate past this level next round
                hyp.resolve(HypothesisResult::Rejected);
                println!(
                    "      Rolling validation → Rejected; escalating past level {}.",
                    proposal.level
                );
            }
            Decision::Inconclusive => {
                level = proposal.level;
                hyp.resolve(HypothesisResult::Inconclusive);
                println!(
                    "      Rolling validation → Inconclusive; escalating past level {}.",
                    proposal.level
                );
            }
        }
        hyps.push(hyp);

        // Non-binding futility check at half budget (PRD 21.1).
        if study.futility_check(matches!(decision, Decision::Accepted)) {
            println!("   Futility check fired; stopping.");
            break;
        }
    }

    println!(
        "   {} hypothesis/es evaluated; {} / {} budget used; best at complexity level {level}.",
        hyps.len(),
        study.consumed(),
        plan.candidate_budget()
    );

    // --- 5. Holdout gate on the accepted candidate (EV-14, 9.6) ---
    let holdout: Vec<Task> = suite.split(Split::Holdout).cloned().collect();
    let mut vault = HoldoutVault::seal(&contract, holdout)?;
    let assurance = CallerIdentity {
        id: "assure".into(),
        plane: Plane::Assurance,
    };
    let base_ref = baseline.clone();
    let best_ref = best.clone();
    let resp = vault.gate_query(
        &assurance,
        &entelechy_artifacts::ArtifactId::of(&best)?.to_string(),
        &contract,
        &move |t| score_one(&base_ref, t),
        &move |t| score_one(&best_ref, t),
    );
    let holdout_summary = match &resp {
        GateResponse::Pass {
            ci_low_pp,
            ci_high_pp,
        } => {
            println!(
                "5. Holdout gate: PASS — improvement 95% interval [{ci_low_pp:.1}, {ci_high_pp:.1}]pp (coarse)."
            );
            serde_json::json!({ "result": "pass", "ci_low_pp": *ci_low_pp, "ci_high_pp": *ci_high_pp })
        }
        GateResponse::Fail {
            ci_low_pp,
            ci_high_pp,
        } => {
            println!("5. Holdout gate: FAIL — [{ci_low_pp:.1}, {ci_high_pp:.1}]pp.");
            serde_json::json!({ "result": "fail", "ci_low_pp": *ci_low_pp, "ci_high_pp": *ci_high_pp })
        }
        GateResponse::Refused { reason } => {
            println!("5. Holdout gate: REFUSED — {reason}");
            serde_json::json!({ "result": "refused", "reason": reason })
        }
    };

    // Persist the final design and a study report (PRD principle 6: artifacts).
    if let Some(dir) = out {
        write_study_outputs(
            dir,
            &plan,
            &baseline_id,
            &best,
            level,
            study.consumed(),
            plan.candidate_budget(),
            &hyps,
            &holdout_summary,
        )?;
    }

    println!(
        "\nEvery accepted mutation has a DesignHypothesis with an experiment result (PRD 25)."
    );
    Ok(())
}

/// Write the final design IR (`design.json`) and a study report
/// (`study-report.json`) to `dir` (PRD principle 6, 21.1). The report records the
/// StudyPlan commitment, the baseline and best content addresses, the final
/// complexity level, budget usage, every hypothesis with its result, and the
/// holdout gate outcome — a reproducible, auditable record of the run.
#[allow(clippy::too_many_arguments)]
fn write_study_outputs(
    dir: &str,
    plan: &StudyPlan,
    baseline_id: &entelechy_artifacts::ArtifactId,
    best: &Program,
    level: u8,
    budget_used: u32,
    budget_total: u32,
    hyps: &[DesignHypothesis],
    holdout: &serde_json::Value,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let best_id = entelechy_artifacts::ArtifactId::of(best)?;
    let report = serde_json::json!({
        "plan_id": plan.id().to_string(),
        "baseline_id": baseline_id.to_string(),
        "best_id": best_id.to_string(),
        "final_complexity_level": level,
        "budget": { "used": budget_used, "total": budget_total },
        "hypotheses": serde_json::to_value(hyps)?,
        "holdout": holdout.clone(),
    });
    let design_path = std::path::Path::new(dir).join("design.json");
    let report_path = std::path::Path::new(dir).join("study-report.json");
    std::fs::write(&design_path, serde_json::to_string_pretty(best)?)?;
    std::fs::write(&report_path, serde_json::to_string_pretty(&report)?)?;
    println!(
        "   Wrote {} (best design {best_id}) and {}.",
        design_path.display(),
        report_path.display()
    );
    Ok(())
}

/// Score a design on every task in a split.
fn score_all(program: &Program, tasks: &[&Task]) -> Vec<bool> {
    tasks.iter().map(|t| score_one(program, t)).collect()
}

/// Turn a design's tune failures into failure observations for the analyzer
/// (PRD 10.2). The two demo failure classes cluster under distinct signatures so
/// the analyzer separates them: an unresolved reply presents as an unsupported
/// claim (→ reasoning.verification, level 3); an unresolved multi-part ticket
/// presents as incomplete decomposition (→ reasoning.decomposition, levels 4/5).
fn observe_failures(program: &Program, tasks: &[&Task]) -> Vec<FailureObservation> {
    tasks
        .iter()
        .filter(|t| !score_one(program, t))
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

/// Simulated helpdesk outcome for a design on one task (declared simulator; see
/// module docs). Two failure classes model the complexity ladder:
/// - a ticket needing a grounded reply is resolved only once the design has a
///   verification step (any `Verify` node — the level-3 operator);
/// - a multi-part ticket needing decomposition is resolved only once the design
///   has a parallel committee (any `Par` node — the level-4 operator).
///
/// The refund negative goal is enforced structurally by the authority envelope.
fn score_one(program: &Program, task: &Task) -> bool {
    // Structural negative goal: refund is forbidden in the envelope, so it can
    // never be violated regardless of the ticket.
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

/// Whether any node in the design satisfies `pred`. The specific node id is
/// irrelevant — the presence of the structure (a Verify or a Par) is what fixes
/// a failure class, so a proposer-generated node (any id) counts.
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

fn frac(outcomes: &[bool]) -> f64 {
    if outcomes.is_empty() {
        0.0
    } else {
        outcomes.iter().filter(|b| **b).count() as f64 / outcomes.len() as f64
    }
}

fn pct(outcomes: &[bool]) -> f64 {
    frac(outcomes) * 100.0
}

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

/// Build the simulated helpdesk suite (hand-authored seeds, PRD Q2), split
/// 12/6/12. Tickets come in two classes: those needing a grounded reply (fixed by
/// a verification step, level 3) and multi-part tickets needing decomposition
/// (fixed by a parallel committee, level 4). Reply tickets outnumber decomposition
/// tickets, so the study fixes verification first and then escalates.
fn helpdesk_suite() -> Suite {
    let mut tasks = Vec::new();
    let mut push = |split: Split, n: usize, tag: &str, base: &str| {
        for i in 0..n {
            tasks.push(Task {
                id: format!("{base}{i}"),
                input: serde_json::json!({ "ticket": format!("{base}{i}"), "class": tag }),
                environment: "helpdesk".into(),
                checkers: vec!["resolved".into()],
                tags: vec![tag.to_string()],
                difficulty: Difficulty::Medium,
                provenance: Provenance::HumanSeed,
                split,
                source_task: None,
            });
        }
    };
    push(Split::Tune, 7, "needs_reply", "tune-reply");
    push(Split::Tune, 5, "needs_decomposition", "tune-dec");
    push(Split::Validation, 3, "needs_reply", "val-reply");
    push(Split::Validation, 3, "needs_decomposition", "val-dec");
    push(Split::Holdout, 7, "needs_reply", "hold-reply");
    push(Split::Holdout, 5, "needs_decomposition", "hold-dec");
    Suite::new(tasks)
}

/// Optional real task synthesis via a live, self-hosted model (EV-3, Q2, 9.5).
///
/// Compiled only under `--features openai`, and active only when both
/// `ENTELECHY_BASE_URL` and `ENTELECHY_MODEL` are set (with an optional
/// `ENTELECHY_API_KEY`). It points [`ModelTaskGenerator`] at an
/// OpenAI-compatible endpoint, checks the Q2 seed-set gate, generates synthetic
/// *tune* variations of a seed, and admits them through the 9.5 contamination
/// firewall — never into the holdout. This is the one real-model path in the
/// otherwise fully simulated demo; a real study must satisfy Q2 before expanding.
#[cfg(feature = "openai")]
fn maybe_synthesize_tasks(suite: &Suite) {
    use entelechy_bench::{admit_candidates, check_seed_set, ModelTaskGenerator};
    use entelechy_gateway::OpenAiGateway;

    let (base, model) = match (
        std::env::var("ENTELECHY_BASE_URL"),
        std::env::var("ENTELECHY_MODEL"),
    ) {
        (Ok(b), Ok(m)) => (b, m),
        _ => return, // not configured: stay on the simulated path
    };

    // Q2 seed-set gate: report sufficiency against the contract's criteria.
    let contract = contract();
    let targets: Vec<&str> = contract.criteria.iter().map(String::as_str).collect();
    let seeds: Vec<Task> = suite.tasks.clone();
    match check_seed_set(&seeds, &targets) {
        Ok(()) => println!("   Task synthesis: Q2 seed set sufficient."),
        Err(defs) => println!(
            "   Task synthesis: Q2 seed set INSUFFICIENT ({} deficiency/ies) — a real study \
             must remedy these before expansion; proceeding to demonstrate wiring: {:?}",
            defs.len(),
            defs
        ),
    }

    let Some(seed) = seeds.iter().find(|t| t.split == Split::Tune) else {
        return;
    };
    let gw = OpenAiGateway::new(base, std::env::var("ENTELECHY_API_KEY").ok());
    let generator = ModelTaskGenerator::new(&gw, model);
    match generator.generate(seed, 5, Split::Tune) {
        Ok(candidates) => {
            let generated = candidates.len();
            match admit_candidates(candidates, suite, Split::Tune) {
                Ok(admitted) => println!(
                    "   Task synthesis (EV-3): generated {generated}, admitted {} synthetic tune \
                     task(s) after 9.5 contamination firewall.",
                    admitted.len()
                ),
                Err(e) => println!("   Task synthesis: admission refused — {e}"),
            }
        }
        Err(e) => println!("   Task synthesis: generation failed — {e}"),
    }
}

/// No-op unless built with `--features openai` (keeps the default study identical).
#[cfg(not(feature = "openai"))]
fn maybe_synthesize_tasks(_suite: &Suite) {}
