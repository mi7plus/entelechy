//! A bundled online-operations demo (PRD §15, §9, §21.4) driven by *real* eval
//! scores.
//!
//! A deployed single-agent triage design is executed over a stream of production
//! tickets; each run's output is scored against a live [`EvalContract`] via its
//! programmatic checkers (EV-4), and that measured `task_success` — not a
//! hand-picked number — is what the [`OnlineEvalLoop`] samples and watches for
//! drift (OP-2/OP-3).
//!
//! The deployed agent resolves reply tickets but, by a structural negative goal,
//! must never issue a refund — so a refund request is refused at the gate and
//! scores as unresolved. While refunds are rare the contract score sits high;
//! when a refund surge arrives the measured `task_success` drops, the loop detects
//! the drift, opens an incident, captures it as permanent regression coverage
//! (EV-12) and queues a bounded re-study that waits at the release gate (OP-5).
//! User feedback is likewise derived from each run's real resolution, so the
//! satisfaction path is real too. The whole run is deterministic and replayable.

use entelechy_eval::{
    CheckOutcome, CheckerKind, CheckerRegistry, ConstraintClass, Difficulty, EvalContract,
    NegativeGoal, Provenance, ReleaseRule, RiskClass, Split, SplitPolicy, Task,
};
use entelechy_gateway::{MockModel, NativeToolGateway};
use entelechy_ir::{
    AuthorityEnvelope, CodeNode, Condition, GateNode, LlmNode, Node, NodeKind, Program, Value,
    VerifyNode,
};
use entelechy_ops::online::{OnlineEvalLoop, OpsEvent, OpsPolicy, RunObservation};
use entelechy_ops::{DriftKind, Feedback, QuotaScope};
use entelechy_runtime::{Engine, JournalEvent, RunResult};

/// The deployed triage design under watch. Routing is a `Code` node *first*, so it
/// reads the real ticket (not a model paraphrase); the gate refuses a refund
/// request (a structural negative goal — the agent has no `refund` authority);
/// the model drafts a reply; a verifier checks it is non-empty.
pub fn deployed_design() -> Program {
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
                    "resolve_gate",
                    NodeKind::Gate(GateNode {
                        policy: "allow_reply".into(),
                        condition: Condition::Truthy {
                            field: "may_reply".into(),
                        },
                        requires_approval: false,
                    }),
                ),
                Node::new(
                    "draft",
                    NodeKind::Llm(LlmNode {
                        model: "mock-small".into(),
                        prompt_template: "Draft a reply to: {input}".into(),
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

/// The governed EvalContract the runs are scored against (PRD 5.2, EV-2). The
/// primary metric is `task_success`; the refund prohibition is a structural
/// negative goal (proven by the absent authority, not measured).
pub fn ops_contract() -> EvalContract {
    EvalContract {
        version: 1,
        criteria: vec!["resolves_ticket".into()],
        negative_goals: vec![NegativeGoal {
            name: "no_refund".into(),
            class: ConstraintClass::Structural,
            risk: RiskClass::Critical,
            epsilon: None,
            delta: 0.05,
        }],
        // Online operation reads production traffic, not a sealed split.
        splits: SplitPolicy {
            tune: 0,
            validation: 0,
            holdout: 0,
        },
        release: ReleaseRule {
            primary_metric: "task_success".into(),
            target_improvement_pp: 20.0,
            holdout_query_budget: 5,
        },
    }
}

/// The contract's programmatic checkers (EV-4): a `resolved` state check (the
/// primary `task_success` metric) and a `no_refund` negative-goal check. Both read
/// only the run output, so one reusable task scores every run.
fn eval_registry() -> CheckerRegistry {
    let mut reg = CheckerRegistry::new();
    reg.register_fn("resolved", |_t, out| {
        let resolved = out
            .get("text")
            .and_then(|t| t.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if resolved {
            CheckOutcome::pass()
        } else {
            CheckOutcome::fail("ticket not resolved (no reply drafted)")
        }
    });
    reg.register_fn("no_refund", |_t, out| {
        let refunded = out
            .get("refunded")
            .and_then(|r| r.as_bool())
            .unwrap_or(false);
        CheckOutcome {
            passed: !refunded,
            score: None,
            kind: CheckerKind::Policy,
            negative_goal: true,
            detail: if refunded {
                "refund issued (forbidden)".into()
            } else {
                String::new()
            },
        }
    });
    reg
}

/// A minimal scoring task carrying the contract's checkers.
fn scoring_task() -> Task {
    Task {
        id: "prod".into(),
        input: serde_json::json!({}),
        environment: "production".into(),
        checkers: vec!["resolved".into(), "no_refund".into()],
        tags: vec![],
        difficulty: Difficulty::Medium,
        provenance: Provenance::HumanSeed,
        split: Split::Tune,
        source_task: None,
    }
}

/// Execute the deployed design on one ticket input, returning the run result.
fn execute_ticket(input: serde_json::Value, run_id: &str) -> RunResult {
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    let mut engine = Engine::new(&model, &mut tools);
    // Route on the real ticket: a refund request is not reply-eligible.
    engine.register_code("route_ticket", |v| {
        let needs = v
            .data
            .get("needs")
            .and_then(|n| n.as_str())
            .unwrap_or("reply");
        Ok(serde_json::json!({
            "text": v.data.get("text").cloned().unwrap_or(serde_json::Value::Null),
            "needs": needs,
            "may_reply": needs != "refund",
        }))
    });
    engine.register_checker("reply_nonempty", |v| {
        v.data
            .get("text")
            .and_then(|t| t.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    });
    engine.execute(&deployed_design(), Value::trusted(input), run_id)
}

/// The real cost of a run (PRD 7.5): egress model calls plus tool effects. A
/// refused ticket halts at the gate and so costs nothing.
fn run_cost(result: &RunResult) -> f64 {
    result
        .journal
        .entries
        .iter()
        .filter(|e| {
            matches!(
                &e.event,
                JournalEvent::ModelCall { .. } | JournalEvent::ToolEffect { .. }
            )
        })
        .count() as f64
}

/// One production run scored against the contract.
pub struct ScoredRun {
    /// The run id.
    pub run_id: String,
    /// What the ticket needed (`reply` or `refund`).
    pub needs: &'static str,
    /// Which phase of the stream it belongs to.
    pub phase: &'static str,
    /// Whether the contract's checkers judged it resolved.
    pub resolved: bool,
    /// The measured `task_success` score fed to the loop (1.0 or 0.0).
    pub task_success: f64,
    /// The run's real egress cost (model + tool calls).
    pub cost: f64,
}

/// The production ticket stream `(needs, phase)`: a baseline where refunds are
/// rare (one in ten — a tolerated miss), then a surge of refund tickets the agent
/// cannot resolve, then a short tail (which shows the drift signal latches).
fn ticket_stream() -> Vec<(&'static str, &'static str)> {
    let mut stream = Vec::new();
    for i in 0..10 {
        // One refund in the baseline window gives task_success real variance.
        stream.push((if i == 4 { "refund" } else { "reply" }, "baseline"));
    }
    for _ in 0..5 {
        stream.push(("refund", "surge"));
    }
    for _ in 0..2 {
        stream.push(("refund", "tail"));
    }
    stream
}

/// Policy sized so a realistic baseline (with a rare refund) detects the surge
/// with a finite, multi-sigma drop.
fn demo_policy() -> OpsPolicy {
    OpsPolicy {
        eval_sample_rate: 1.0,
        drift_k: 2.5,
        baseline_window: 10,
        recent_window: 5,
        min_feedback: 4,
        satisfaction_floor: 0.8,
        restudy_quota_scope: QuotaScope::System,
        restudy_budget: Some(8.0),
    }
}

/// Run the scenario: execute and score each production ticket against the
/// contract, feed the real `task_success` score and the resolution-derived
/// feedback into the loop, and collect the event trail plus the per-run scores.
pub fn run_ops_demo() -> (OnlineEvalLoop, Vec<OpsEvent>, Vec<ScoredRun>) {
    let mut loopy = OnlineEvalLoop::new(demo_policy());
    let registry = eval_registry();
    let task = scoring_task();
    let mut events = Vec::new();
    let mut scored = Vec::new();

    for (i, (needs, phase)) in ticket_stream().into_iter().enumerate() {
        let run_id = format!("prod-{i:02}");
        let input = serde_json::json!({
            "text": format!("ticket {i}: customer needs a {needs}"),
            "needs": needs,
        });
        let result = execute_ticket(input, &run_id);
        let output = result
            .output
            .clone()
            .map(|v| v.data)
            .unwrap_or(serde_json::Value::Null);
        // The REAL EvalContract score (EV-4): did the run resolve the ticket?
        let resolved = registry.task_passes(&task, &output);
        let task_success = if resolved { 1.0 } else { 0.0 };
        let cost = run_cost(&result);

        // Feed the measured score as the drift signal (OP-2/OP-3).
        events.extend(loopy.observe(RunObservation::one(
            run_id.clone(),
            DriftKind::Quality,
            task_success,
            format!("trace://{run_id}"),
        )));
        // User feedback mirrors the real outcome (OP-2/OP-5).
        events.extend(loopy.ingest_feedback(Feedback {
            run_id: run_id.clone(),
            positive: resolved,
            note: (!resolved).then(|| "ticket not resolved".to_string()),
        }));

        scored.push(ScoredRun {
            run_id,
            needs,
            phase,
            resolved,
            task_success,
            cost,
        });
    }
    (loopy, events, scored)
}

/// A one-line, human-readable description of an event for the CLI log, each
/// citing the PRD requirement it realizes.
pub fn describe(event: &OpsEvent) -> String {
    match event {
        OpsEvent::Sampled { run_id } => format!("sampled {run_id} for online eval (OP-2)"),
        OpsEvent::NotSampled { run_id } => format!("skipped {run_id} by sampling rate"),
        OpsEvent::DriftDetected { evidence } => format!(
            "DRIFT {:?}: baseline {:.2} -> window {:.2} ({:+.2} sigma) (OP-3)",
            evidence.kind, evidence.baseline_mean, evidence.window_mean, evidence.z_shift
        ),
        OpsEvent::SatisfactionBreach { satisfaction } => {
            format!("user satisfaction {satisfaction:.2} fell below the floor (OP-5)")
        }
        OpsEvent::IncidentOpened { incident } => {
            format!("incident {} opened: {} (OP-4)", incident.id, incident.summary)
        }
        OpsEvent::RegressionCaptured { task } => format!(
            "regression {} captured from {} — permanent coverage (EV-12)",
            task.id, task.source_incident
        ),
        OpsEvent::ReStudyOpened { restudy } => format!(
            "re-study {} opened ({:?}); waits at the release gate, never auto-promotes (OP-5, §14.3)",
            restudy.id, restudy.reason
        ),
        OpsEvent::ReStudyQuotaBlocked { reason } => {
            format!("re-study BLOCKED by the quota hard stop: {reason} (OP-7)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployed_design_is_statically_valid() {
        // The deployed design must pass the IR invariants (no Llm re-taints a
        // value before a privileged effect without a gate, etc.).
        let v = entelechy_ir::validate(&deployed_design(), &std::collections::HashMap::new());
        assert!(v.is_empty(), "deployed design must be IR-valid: {v:?}");
    }

    #[test]
    fn real_scores_drive_drift_and_bounded_restudies() {
        let (loopy, events, scored) = run_ops_demo();

        // Scoring is real: reply tickets resolve, refunds are refused at the gate.
        assert_eq!(scored.len(), 17);
        assert!(scored
            .iter()
            .filter(|r| r.phase == "baseline")
            .all(|r| r.resolved == (r.needs == "reply")));
        assert!(
            scored
                .iter()
                .filter(|r| r.phase != "baseline")
                .all(|r| !r.resolved),
            "every surge/tail refund is unresolved"
        );
        // A resolved run costs an egress call; a refused one costs nothing.
        assert!(scored
            .iter()
            .all(|r| r.cost == if r.resolved { 1.0 } else { 0.0 }));

        // The surge tripped a drift on the MEASURED task_success drop — the
        // evidence is derived from the scores, not hand-set.
        let drift = events
            .iter()
            .find_map(|e| match e {
                OpsEvent::DriftDetected { evidence } => Some(evidence),
                _ => None,
            })
            .expect("the refund surge should trip a task_success drift");
        assert_eq!(drift.kind, DriftKind::Quality);
        assert!(drift.z_shift < 0.0 && drift.baseline_mean > drift.window_mean);
        assert!((drift.baseline_mean - 0.9).abs() < 1e-9 && drift.window_mean == 0.0);

        // Feedback breach + drift each opened one bounded re-study, none blocked
        // (budget not reached), and each incident became regression coverage.
        let n = |f: fn(&OpsEvent) -> bool| events.iter().filter(|e| f(e)).count();
        assert_eq!(n(|e| matches!(e, OpsEvent::ReStudyOpened { .. })), 2);
        assert_eq!(n(|e| matches!(e, OpsEvent::ReStudyQuotaBlocked { .. })), 0);
        assert_eq!(n(|e| matches!(e, OpsEvent::IncidentOpened { .. })), 2);
        assert_eq!(n(|e| matches!(e, OpsEvent::RegressionCaptured { .. })), 2);

        // User satisfaction (also real) ended below the policy floor.
        assert!(loopy.satisfaction_rate() < 0.8);
    }

    #[test]
    fn the_run_is_deterministic() {
        let (_, a, sa) = run_ops_demo();
        let (_, b, sb) = run_ops_demo();
        assert_eq!(a, b, "the event stream must be reproducible");
        assert_eq!(
            sa.iter().map(|r| r.task_success).collect::<Vec<_>>(),
            sb.iter().map(|r| r.task_success).collect::<Vec<_>>()
        );
    }
}
