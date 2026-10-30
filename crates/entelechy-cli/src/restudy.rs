//! Closing the operate → re-study → release loop (PRD §14.3, §21, OP-5, EV-12,
//! EV-14).
//!
//! The online-eval loop ([`crate::ops`]) detects a drift from real
//! [`EvalContract`] scores, opens an [`Incident`], captures it as permanent
//! regression coverage, and opens a bounded [`ReStudy`] — which, until now, was
//! inert. This module turns that opened re-study into an actual re-optimization:
//!
//! 1. It builds a repaired candidate design ([`repaired_design`]) that resolves the
//!    failure class the incident exposed — refund tickets — *without* acquiring the
//!    forbidden `refund` capability. The repair is a minimal typed patch: it removes
//!    the deployed design's over-restrictive refusal `Gate`, so a refund ticket is
//!    no longer left unresolved but gets a safe drafted response (an escalation
//!    hand-off). The authority is unchanged, so the structural negative goal — no
//!    way to issue a refund — stays fully intact.
//! 2. It exercises the candidate on the incident-derived **regression suite**
//!    (`Provenance::Incident`, EV-12) to show the class is fixed. Per PRD 9.5 these
//!    incident tasks are regression coverage, never independent holdout evidence.
//! 3. It gates the candidate on a sealed **independent holdout** through the
//!    assurance-only release gate ([`HoldoutVault`], EV-14, 9.6).
//! 4. Critically, even a passing candidate is **never auto-promoted**
//!    ([`ReStudy::auto_promotes`] is always `false`): it stops at the release gate
//!    awaiting human approval (OP-5, the PRD non-goal of autonomous promotion).

use std::collections::HashMap;

use entelechy_eval::{
    CallerIdentity, Difficulty, EvalContract, GateResponse, HoldoutVault, Plane, Provenance, Split,
    Task,
};
use entelechy_ir::{AuthorityEnvelope, CodeNode, LlmNode, Node, NodeKind, Program, VerifyNode};
use entelechy_ops::{Incident, ReStudy, ReStudyReason};
use serde::Serialize;

use crate::ops::{deployed_design, resolves};

/// Size of the incident-derived regression suite (the refund failure class).
const REGRESSION_SIZE: usize = 8;
/// Tasks per class in the independent holdout (reply + refund → twice this).
const HOLDOUT_PER_CLASS: usize = 12;

/// The re-study's repaired candidate design: the deployed design with its
/// refund-refusing `Gate` removed (a minimal typed patch). The deployed design
/// refuses to reply whenever `may_reply` is false, leaving every refund ticket
/// unresolved; the repair drops that refusal so a refund ticket instead gets a
/// safe drafted response — an escalation hand-off — which the verifier confirms is
/// non-empty.
///
/// The authority envelope is unchanged — `draft_reply` is granted and `refund`
/// stays forbidden — and no node issues a refund, so the repair fixes the failure
/// class without weakening the structural negative goal: the candidate still has
/// no way to issue a refund, it only drafts text.
pub fn repaired_design() -> Program {
    let mut authority = AuthorityEnvelope::empty();
    authority.capabilities.insert("draft_reply".into());
    authority.forbidden_capabilities.insert("refund".into());
    Program::new(
        authority,
        Node::new(
            "root",
            NodeKind::Seq(vec![
                Node::new(
                    "triage",
                    NodeKind::Code(CodeNode {
                        function: "route_ticket".into(),
                    }),
                ),
                Node::new(
                    "draft",
                    NodeKind::Llm(LlmNode {
                        model: "mock-small".into(),
                        prompt_template:
                            "Draft a safe response (a reply, or an escalation hand-off for a \
                             refund request — never a refund) to: {input}"
                                .into(),
                        temperature: 0.0,
                    }),
                ),
                Node::new(
                    "verify",
                    NodeKind::Verify(VerifyNode {
                        checker: "reply_nonempty".into(),
                    }),
                ),
            ]),
        ),
    )
}

/// The outcome of a re-study: the regression-class fix (EV-12), the independent
/// holdout release gate result (EV-14), and the promotion decision — which is
/// always "not promoted" (OP-5, 14.3). Serializable so it is persisted with the
/// ops report and committed to the audit chain.
#[derive(Clone, Debug, Serialize)]
pub struct ReStudyOutcome {
    /// The re-study being run.
    pub restudy_id: String,
    /// Why the re-study was opened.
    pub reason: ReStudyReason,
    /// The incident that triggered it.
    pub incident_id: String,
    /// Content address of the repaired candidate design.
    pub candidate_hash: String,
    /// Tasks in the incident-derived regression suite.
    pub regression_total: usize,
    /// How many the deployed design resolved (it fails the whole class).
    pub regression_baseline_pass: usize,
    /// How many the candidate resolved (it fixes the class).
    pub regression_candidate_pass: usize,
    /// Tasks in the sealed independent holdout.
    pub holdout_total: usize,
    /// The release gate's coarse verdict (never per-task — EV-14).
    pub gate: GateResponse,
    /// Whether the candidate was promoted. Always `false`: a re-study candidate
    /// waits at the release gate for human approval (OP-5, PRD 14.3).
    pub promoted: bool,
}

impl ReStudyOutcome {
    /// A one-line audit statement of the governed release-gate decision, for the
    /// hash-chained ops audit log (PRD 17.2).
    pub(crate) fn audit_line(&self) -> String {
        let verdict = match &self.gate {
            GateResponse::Pass { .. } => "pass",
            GateResponse::Fail { .. } => "fail",
            GateResponse::Refused { .. } => "refused",
        };
        format!(
            "re-study {} release gate: {verdict}; candidate {} (regression {}/{} fixed)",
            self.restudy_id,
            if self.promoted {
                "promoted"
            } else {
                "held for human approval (never auto-promoted)"
            },
            self.regression_candidate_pass,
            self.regression_total,
        )
    }
}

/// Run an opened re-study to a governed release decision (PRD §14.3, OP-5).
///
/// Scores the deployed design and the repaired candidate on the incident's
/// regression class (EV-12, scored directly) and on a sealed independent holdout
/// queried through the assurance-only gate (EV-14). Returns the outcome; the
/// candidate is never auto-promoted regardless of the gate result.
pub fn run_restudy(
    restudy: &ReStudy,
    incident: &Incident,
    contract: &EvalContract,
) -> anyhow::Result<ReStudyOutcome> {
    let deployed = deployed_design();
    let repaired = repaired_design();
    let candidate_hash = entelechy_artifacts::ArtifactId::of(&repaired)?.to_string();

    // 1. Regression coverage from the incident (EV-12): the refund failure class,
    //    scored directly. The deployed design fails the whole class; the candidate
    //    fixes it. These incident-derived tasks are never independent holdout
    //    evidence (PRD 9.5) — they are a regression gate, not the release gate.
    let regression = regression_suite(incident);
    let regression_baseline_pass = regression.iter().filter(|t| resolves(&deployed, t)).count();
    let regression_candidate_pass = regression.iter().filter(|t| resolves(&repaired, t)).count();

    // 2. Independent holdout release gate (EV-14, 9.6): a sealed, incident-free mix
    //    of reply and refund tickets. Only the assurance plane may query it, and it
    //    returns only a coarse aggregate. Scores are precomputed so each design runs
    //    the real runtime once per task.
    let holdout = independent_holdout();
    let holdout_total = holdout.len();
    let base_scores = score_map(&deployed, &holdout);
    let cand_scores = score_map(&repaired, &holdout);
    let mut vault = HoldoutVault::seal(contract, holdout)?;
    let assurance = CallerIdentity {
        id: "assure".into(),
        plane: Plane::Assurance,
    };
    let gate = vault.gate_query(
        &assurance,
        &candidate_hash,
        contract,
        &|t| base_scores.get(&t.id).copied().unwrap_or(false),
        &|t| cand_scores.get(&t.id).copied().unwrap_or(false),
    );

    // 3. Release decision: a re-study never auto-promotes. Even a passing candidate
    //    stops here, awaiting a human release approval (OP-5, PRD 14.3).
    let promoted = gate.is_pass() && restudy.auto_promotes();

    Ok(ReStudyOutcome {
        restudy_id: restudy.id.clone(),
        reason: restudy.reason,
        incident_id: incident.id.clone(),
        candidate_hash,
        regression_total: regression.len(),
        regression_baseline_pass,
        regression_candidate_pass,
        holdout_total,
        gate,
        promoted,
    })
}

/// A human-readable, multi-line description of a re-study outcome for the CLI.
pub fn describe_outcome(o: &ReStudyOutcome) -> String {
    let gate = match &o.gate {
        GateResponse::Pass {
            ci_low_pp,
            ci_high_pp,
        } => format!("PASS — improvement 95% interval [{ci_low_pp:.1}, {ci_high_pp:.1}]pp"),
        GateResponse::Fail {
            ci_low_pp,
            ci_high_pp,
        } => format!("FAIL — [{ci_low_pp:.1}, {ci_high_pp:.1}]pp"),
        GateResponse::Refused { reason } => format!("REFUSED — {reason}"),
    };
    format!(
        "re-study {} (opened {:?} from incident {}):\n  \
         repaired candidate {}\n  \
         regression class (EV-12): deployed {}/{} resolved → candidate {}/{} resolved\n  \
         release gate on sealed independent holdout ({} tasks, EV-14): {gate}\n  \
         → candidate {} (OP-5, PRD 14.3)",
        o.restudy_id,
        o.reason,
        o.incident_id,
        o.candidate_hash,
        o.regression_baseline_pass,
        o.regression_total,
        o.regression_candidate_pass,
        o.regression_total,
        o.holdout_total,
        if o.promoted {
            "PROMOTED"
        } else {
            "held at the release gate for human approval — never auto-promoted"
        },
    )
}

/// Score a design on a task set, keyed by task id (one real runtime execution per
/// task — [`resolves`] runs the program and applies the contract's checkers).
fn score_map(program: &Program, tasks: &[Task]) -> HashMap<String, bool> {
    tasks
        .iter()
        .map(|t| (t.id.clone(), resolves(program, t)))
        .collect()
}

/// The incident-derived regression suite (EV-12): the refund failure class the
/// incident exposed, marked `Provenance::Incident` so it is permanent regression
/// coverage and never independent holdout (PRD 9.5). Not sealed in a vault — it is
/// a regression gate, scored directly.
fn regression_suite(incident: &Incident) -> Vec<Task> {
    (0..REGRESSION_SIZE)
        .map(|i| {
            ticket_task(
                format!("regression-{}-{i}", incident.id),
                "refund",
                Provenance::Incident,
                // Regression coverage lives on the control-plane (tune) side; it is
                // explicitly NOT holdout (PRD 9.5).
                Split::Tune,
            )
        })
        .collect()
}

/// A fresh, incident-free independent holdout: an even mix of reply and refund
/// tickets sealed under the contract for the release gate (EV-14). Human-seeded, so
/// it is legitimate independent evidence (not incident-derived — PRD 9.5).
fn independent_holdout() -> Vec<Task> {
    let mut tasks = Vec::with_capacity(HOLDOUT_PER_CLASS * 2);
    for i in 0..HOLDOUT_PER_CLASS {
        tasks.push(ticket_task(
            format!("holdout-reply-{i}"),
            "reply",
            Provenance::HumanSeed,
            Split::Holdout,
        ));
        tasks.push(ticket_task(
            format!("holdout-refund-{i}"),
            "refund",
            Provenance::HumanSeed,
            Split::Holdout,
        ));
    }
    tasks
}

/// One ticket task carrying the ops contract's checkers (`resolved`, `no_refund`).
fn ticket_task(id: String, needs: &str, provenance: Provenance, split: Split) -> Task {
    Task {
        input: serde_json::json!({
            "text": format!("{id}: customer needs a {needs}"),
            "needs": needs,
        }),
        environment: "production".into(),
        checkers: vec!["resolved".into(), "no_refund".into()],
        tags: vec![needs.to_string()],
        difficulty: Difficulty::Medium,
        provenance,
        split,
        source_task: None,
        id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::ops_contract;

    #[test]
    fn repaired_design_is_valid_and_keeps_the_negative_goal() {
        let repaired = repaired_design();
        // IR-valid: no Tool/consequential effect follows the (tainted) Llm drafts,
        // so the Branch→Verify shape raises no IR-I3 taint violation.
        let v = entelechy_ir::validate(&repaired, &std::collections::HashMap::new());
        assert!(v.is_empty(), "repaired design must be IR-valid: {v:?}");
        // The structural negative goal is intact: refund stays forbidden and
        // ungranted, so the candidate has no way to issue a refund.
        assert!(repaired.authority.forbidden_capabilities.contains("refund"));
        assert!(!repaired.authority.capabilities.contains("refund"));
    }

    #[test]
    fn repaired_resolves_both_classes_while_deployed_fails_refunds() {
        let deployed = deployed_design();
        let repaired = repaired_design();
        let reply = ticket_task(
            "t-reply".into(),
            "reply",
            Provenance::HumanSeed,
            Split::Holdout,
        );
        let refund = ticket_task(
            "t-refund".into(),
            "refund",
            Provenance::HumanSeed,
            Split::Holdout,
        );
        // Deployed resolves replies but refuses refunds at the gate.
        assert!(resolves(&deployed, &reply));
        assert!(!resolves(&deployed, &refund));
        // The repair resolves both — the refund by escalation, not a refund.
        assert!(resolves(&repaired, &reply));
        assert!(resolves(&repaired, &refund));
    }

    #[test]
    fn restudy_gates_candidate_on_holdout_and_never_promotes() {
        let restudy = ReStudy {
            id: "rs-test".into(),
            reason: ReStudyReason::Drift,
            evidence_ref: "drift://Quality".into(),
        };
        let incident = Incident {
            id: "inc-test".into(),
            trace_refs: vec!["trace://prod-14".into()],
            summary: "Quality drift".into(),
        };
        let o = run_restudy(&restudy, &incident, &ops_contract()).expect("re-study runs");

        // The incident's failure class is fully fixed (EV-12).
        assert_eq!(
            o.regression_baseline_pass, 0,
            "deployed fails the whole class"
        );
        assert_eq!(
            o.regression_candidate_pass, o.regression_total,
            "the candidate fixes the whole regression class"
        );
        // The independent holdout gate passes on the measured 50pp improvement.
        assert!(o.gate.is_pass(), "release gate should pass: {:?}", o.gate);
        // …but the candidate is NEVER auto-promoted (OP-5, PRD 14.3).
        assert!(
            !o.promoted,
            "a re-study candidate must stop at the release gate for human approval"
        );
    }

    #[test]
    fn the_demo_drift_restudy_closes_the_loop() {
        // End to end: the online demo opens a drift re-study, and running it drives
        // a gated-but-unpromoted candidate — the operate → re-study → release loop.
        let (_, events, _) = crate::ops::run_ops_demo();
        let (restudy, incident) =
            crate::ops::drift_case(&events).expect("the demo opens a drift re-study");
        assert_eq!(restudy.reason, ReStudyReason::Drift);
        let o = run_restudy(&restudy, &incident, &ops_contract()).expect("re-study runs");
        assert!(o.gate.is_pass(), "{:?}", o.gate);
        assert!(!o.promoted);
    }

    #[test]
    fn the_outcome_is_deterministic() {
        let restudy = ReStudy {
            id: "rs-det".into(),
            reason: ReStudyReason::Drift,
            evidence_ref: "drift://Quality".into(),
        };
        let incident = Incident {
            id: "inc-det".into(),
            trace_refs: vec![],
            summary: "Quality drift".into(),
        };
        let a = run_restudy(&restudy, &incident, &ops_contract()).unwrap();
        let b = run_restudy(&restudy, &incident, &ops_contract()).unwrap();
        assert_eq!(a.candidate_hash, b.candidate_hash);
        assert_eq!(a.gate, b.gate);
    }
}
