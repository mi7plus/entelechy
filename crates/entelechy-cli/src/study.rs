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

use entelechy_bench::{BenchmarkManifest, StoppingRule, StudyPlan};
use entelechy_design::{
    apply, synthesize_single_agent, DesignHypothesis, EditOp, HypothesisResult,
};
use entelechy_eval::{
    CallerIdentity, ConstraintClass, Difficulty, EvalContract, GateResponse, HoldoutVault,
    NegativeGoal, Plane, Provenance, ReleaseRule, RiskClass, Split, SplitPolicy, Suite, Task,
};
use entelechy_failure::{FailureObservation, Symptom};
use entelechy_ir::{AuthorityEnvelope, Node, NodeKind, Program};
use entelechy_search::{rolling_validation_decision, Candidate, Decision, Study};

/// Run the Phase 0 study demo, printing a narrative of each governed step.
pub fn run() -> anyhow::Result<()> {
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

    // --- 4. Repair loop: one hypothesis, accounted and confirmed (EV-13) ---
    let mut study = Study::from_plan(&plan);
    let tune: Vec<&Task> = suite.split(Split::Tune).collect();
    let val: Vec<&Task> = suite.split(Split::Validation).collect();

    let base_tune = score_all(&baseline, &tune);
    let base_val = score_all(&baseline, &val);
    println!(
        "4. Baseline success — tune {:.0}%, validation {:.0}%.",
        pct(&base_tune),
        pct(&base_val)
    );

    // 4a. Diagnose: cluster the baseline's tune failures (PRD 10.2, Appendix B).
    let observations = observe_failures(&baseline, &tune);
    let clusters = entelechy_failure::analyze(&observations);
    let top = clusters
        .iter()
        .find(|c| !c.is_evaluation_failure)
        .and_then(|c| c.top().map(|t| (c, t)));
    let (evidence, failure_class, confidence, patch) = match top {
        Some((cluster, cls)) => {
            println!(
                "   Diagnosis: cluster '{}' — {} ({} tasks, confidence {:.0}%, eligible levels {:?}).",
                cluster.id,
                cls.class_id,
                cluster.members.len(),
                cls.probability * 100.0,
                cls.eligible_levels
            );
            (
                format!("cluster '{}': {} tasks", cluster.id, cluster.members.len()),
                cls.class_id.clone(),
                cls.probability,
                patch_for_class(&cls.class_id),
            )
        }
        None => {
            println!("   Diagnosis: no actionable failure cluster.");
            ("no failures".into(), "none".into(), 1.0, vec![])
        }
    };

    // Form a DesignHypothesis from the diagnosis (PRD 11.2, principle 5).
    let mut h1 = DesignHypothesis {
        id: "H-1".into(),
        evidence,
        failure_class,
        suspected_cause: "no post-synthesis verification of the reply".into(),
        cause_confidence: confidence,
        patch,
        expected_effect: "raise task success on reply tasks".into(),
        expected_tradeoff: "small latency/cost increase".into(),
        experiment: "paired eval vs baseline on tune, confirm on validation".into(),
        result: HypothesisResult::Pending,
    };

    // Apply the smallest compiler-valid patch (PRD 11.3).
    let mut candidate = baseline.clone();
    for op in &h1.patch {
        candidate = apply(&candidate, op)?;
    }
    // Account every evaluated candidate against the budget (PRD 21.1).
    study.account_candidate()?;
    let cand_tune = score_all(&candidate, &tune);
    let cand_val = score_all(&candidate, &val);
    println!(
        "   Candidate (H-1 applied) success — tune {:.0}%, validation {:.0}%.",
        pct(&cand_tune),
        pct(&cand_val)
    );

    // Rolling-validation decision (EV-13): confirm on fresh validation tasks.
    let decision =
        rolling_validation_decision(&base_tune, &cand_tune, &base_val, &cand_val, 20250920);
    h1.resolve(match decision {
        Decision::Accepted => HypothesisResult::Accepted,
        Decision::Rejected => HypothesisResult::Rejected,
        Decision::Inconclusive => HypothesisResult::Inconclusive,
    });
    println!(
        "   Rolling validation → {decision:?}; hypothesis {} result {:?}.",
        h1.id, h1.result
    );

    let best = if decision == Decision::Accepted {
        study.accept(Candidate {
            design_hash: entelechy_artifacts::ArtifactId::of(&candidate)?.to_string(),
            validation_success: frac(&cand_val),
        });
        &candidate
    } else {
        &baseline
    };
    println!(
        "   Candidates evaluated: {} / budget {} (remaining {}).",
        study.consumed(),
        plan.candidate_budget(),
        study.remaining()
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
        &entelechy_artifacts::ArtifactId::of(best)?.to_string(),
        &contract,
        &move |t| score_one(&base_ref, t),
        &move |t| score_one(&best_ref, t),
    );
    match resp {
        GateResponse::Pass { ci_low_pp, ci_high_pp } => println!(
            "5. Holdout gate: PASS — improvement 95% interval [{ci_low_pp:.1}, {ci_high_pp:.1}]pp (coarse)."
        ),
        GateResponse::Fail { ci_low_pp, ci_high_pp } => println!(
            "5. Holdout gate: FAIL — [{ci_low_pp:.1}, {ci_high_pp:.1}]pp."
        ),
        GateResponse::Refused { reason } => println!("5. Holdout gate: REFUSED — {reason}"),
    }

    println!(
        "\nEvery accepted mutation has a DesignHypothesis with an experiment result (PRD 25)."
    );
    Ok(())
}

/// Score a design on every task in a split.
fn score_all(program: &Program, tasks: &[&Task]) -> Vec<bool> {
    tasks.iter().map(|t| score_one(program, t)).collect()
}

/// Turn a design's tune failures into failure observations for the analyzer
/// (PRD 10.2). A ticket that needs a grounded reply but is unresolved presents as
/// an unsupported-claim symptom (→ reasoning.verification).
fn observe_failures(program: &Program, tasks: &[&Task]) -> Vec<FailureObservation> {
    tasks
        .iter()
        .filter(|t| !score_one(program, t))
        .map(|t| FailureObservation {
            task_id: t.id.clone(),
            signature: "reply:unverified".into(),
            symptom: if t.has_tag("needs_reply") {
                Symptom::UnsupportedClaim
            } else {
                Symptom::Unknown
            },
            is_evaluation_failure: false,
        })
        .collect()
}

/// Choose the smallest typed patch for a diagnosed failure class (PRD 11.3/11.5).
/// The class's eligible complexity level (PRD 11.4) picks the operator family; in
/// this Phase 0 slice the verification level maps to adding a Verify step.
fn patch_for_class(class_id: &str) -> Vec<EditOp> {
    match class_id {
        "reasoning.verification" | "reasoning.synthesis" | "knowledge.grounding_failure" => {
            vec![EditOp::AddVerify {
                id: "reply_check".into(),
                checker: "reply_supported".into(),
            }]
        }
        // Other classes would map to their own operators (change model, add
        // retrieval, etc.) as those unlock; default to a verification step.
        _ => vec![EditOp::AddVerify {
            id: "reply_check".into(),
            checker: "reply_supported".into(),
        }],
    }
}

/// Simulated helpdesk outcome for a design on one task (declared simulator; see
/// module docs). A ticket that needs a grounded reply is resolved only when the
/// design contains the `reply_check` verification step; the refund negative goal
/// is enforced structurally by the authority envelope.
fn score_one(program: &Program, task: &Task) -> bool {
    let needs_reply = task.has_tag("needs_reply");
    let has_verify = contains_node(&program.root, "reply_check");
    // Structural negative goal: refund is forbidden in the envelope, so it can
    // never be violated regardless of the ticket.
    let refund_safe = program.authority.forbidden_capabilities.contains("refund");
    if !refund_safe {
        return false;
    }
    if needs_reply {
        has_verify
    } else {
        true
    }
}

fn contains_node(node: &Node, id: &str) -> bool {
    if node.id == id {
        return true;
    }
    match &node.kind {
        NodeKind::Seq(c) | NodeKind::Par(c) => c.iter().any(|n| contains_node(n, id)),
        NodeKind::Map { body, .. }
        | NodeKind::Loop { body, .. }
        | NodeKind::Delegate { body, .. } => contains_node(body, id),
        NodeKind::Branch { then, els, .. } => contains_node(then, id) || contains_node(els, id),
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

/// Build the simulated helpdesk suite: a mix of reply-needing and simple tickets,
/// hand-authored seeds (PRD Q2), split 12/6/12.
fn helpdesk_suite() -> Suite {
    let mut tasks = Vec::new();
    let mut push = |split: Split, n: usize, base: &str| {
        for i in 0..n {
            // Alternate: even tickets need a grounded reply, odd are simple acks.
            let needs_reply = i % 2 == 0;
            let mut tags = vec![];
            if needs_reply {
                tags.push("needs_reply".to_string());
            }
            tasks.push(Task {
                id: format!("{base}{i}"),
                input: serde_json::json!({ "ticket": format!("{base}{i}"), "needs_reply": needs_reply }),
                environment: "helpdesk".into(),
                checkers: vec!["resolved".into()],
                tags,
                difficulty: Difficulty::Medium,
                provenance: Provenance::HumanSeed,
                split,
                source_task: None,
            });
        }
    };
    push(Split::Tune, 12, "tune");
    push(Split::Validation, 6, "val");
    push(Split::Holdout, 12, "hold");
    Suite::new(tasks)
}
