//! A minimal end-to-end demo: the Phase 0 support-triage shape in miniature
//! (PRD 21, Q1). It builds a single-agent IR, executes it against the mock model
//! and a simulated helpdesk tool, then replays the recorded journal to show
//! deterministic replay (PRD 8.2, 18).

use entelechy_gateway::{MockModel, NativeToolGateway};
use entelechy_ir::{
    AttestationLevel, AuthorityEnvelope, CodeNode, Condition, EffectClass, EffectMetadata,
    GateNode, LlmNode, Node, NodeKind, Program, ToolNode, VerifyNode,
};
use entelechy_policy::{NativePolicy, PolicySnapshot};
use entelechy_runtime::{Engine, PolicyConfig, RunResult, RunStatus};
use std::collections::HashMap;

/// Build the demo single-agent program.
///
/// Shape: classify the ticket with the model, decide whether it is a refund
/// request, gate the (forbidden-by-default) refund path, otherwise draft a reply
/// via a simulated tool, then verify the draft is non-empty.
pub fn demo_program() -> Program {
    // Authority: may draft replies, must never refund (PRD 21 negative goal).
    let mut authority = AuthorityEnvelope::empty();
    authority.capabilities.insert("draft_reply".into());
    authority.forbidden_capabilities.insert("refund".into());

    Program::new(
        authority,
        Node::new(
            "root",
            NodeKind::Seq(vec![
                Node::new(
                    "classify",
                    NodeKind::Llm(LlmNode {
                        model: "mock-small".into(),
                        prompt_template: "Classify this ticket and respond: {input}".into(),
                        temperature: 0.0,
                    }),
                ),
                Node::new(
                    "route",
                    NodeKind::Code(CodeNode {
                        function: "route_ticket".into(),
                    }),
                ),
                // Gate guards the transition to a consequential tool effect.
                Node::new(
                    "reply_gate",
                    NodeKind::Gate(GateNode {
                        policy: "allow_draft".into(),
                        condition: Condition::Truthy {
                            field: "may_reply".into(),
                        },
                        requires_approval: false,
                    }),
                ),
                Node::new(
                    "draft",
                    NodeKind::Tool(ToolNode {
                        capability: "draft_reply".into(),
                        args: serde_json::json!({ "channel": "email" }),
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

/// The capability catalog for the demo (PRD 7.6 effect metadata).
pub fn demo_catalog() -> HashMap<String, EffectMetadata> {
    let mut c = HashMap::new();
    c.insert(
        "draft_reply".to_string(),
        EffectMetadata {
            class: EffectClass::Write,
            idempotent: true,
            reversible: true,
            dry_run_supported: true,
            read_back_supported: true,
            attestation: AttestationLevel::OperatorAttested,
            operation_key_namespace: "helpdesk.draft".into(),
        },
    );
    c
}

/// Run the demo end to end, returning the execution result and the number of
/// policy decisions recorded at effect boundaries (PRD 8.3).
pub fn run_demo(run_id: &str) -> (RunResult, usize) {
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    // Simulated helpdesk draft tool with resettable, deterministic behavior.
    tools.register("draft_reply", |args| {
        let channel = args
            .get("channel")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        Ok(serde_json::json!({
            "reply": format!("Thanks for reaching out via {channel}; we're looking into it."),
            "sent": false
        }))
    });

    // Enforce authority at effect boundaries (PRD 8.3): draft_reply is granted,
    // refund is forbidden by the envelope.
    let mut engine = Engine::new(&model, &mut tools).with_policy(PolicyConfig {
        engine: Box::new(NativePolicy::new()),
        authority: demo_program().authority.clone(),
        snapshot: PolicySnapshot::default(),
        catalog: demo_catalog(),
        principal: "runtime".into(),
    });
    // route_ticket: mark the ticket as reply-eligible unless it is a refund.
    engine.register_code("route_ticket", |v| {
        let text = v.data.get("text").and_then(|t| t.as_str()).unwrap_or("");
        let is_refund = text.to_lowercase().contains("refund");
        Ok(serde_json::json!({
            "text": text,
            "is_refund": is_refund,
            "may_reply": !is_refund,
        }))
    });
    engine.register_checker("reply_nonempty", |v| {
        v.data
            .get("reply")
            .and_then(|r| r.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    });

    let result = engine.execute(
        &demo_program(),
        entelechy_ir::Value::trusted(serde_json::json!({})),
        run_id,
    );
    let decisions = engine.policy_decisions().entries.len();
    (result, decisions)
}

/// A short human-readable status line for a run result.
pub fn status_line(result: &RunResult) -> String {
    match &result.status {
        RunStatus::Succeeded => format!(
            "succeeded ({} journal entries)",
            result.journal.entries.len()
        ),
        RunStatus::Failed { node_path, reason } => {
            format!("failed at {node_path}: {reason:?}")
        }
        RunStatus::BudgetExhausted { budget } => format!("budget exhausted: {budget}"),
        RunStatus::ReconciliationRequired => "reconciliation required".to_string(),
    }
}
