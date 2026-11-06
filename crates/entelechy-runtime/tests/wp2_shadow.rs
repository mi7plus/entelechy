//! WP2 shadow-enforcement red-team suite (PRD §14.2 L3).
//!
//! Adversarial, public-API probes that a shadow run is a *runtime* effect-substitution
//! guarantee, not a governance layer that trusts `dry_run_supported` metadata. In
//! shadow the engine runs the whole program — journaling, authority narrowing (WS2),
//! budget (WS3) and information-flow labels (WS1) all enforced — but never calls the
//! tool gateway for any consequential `Tool` node: the effect is authorized and
//! budgeted, then substituted (recorded, given a record-only response). So a shadow
//! run commits nothing regardless of what the capability's metadata claims, which the
//! staged-rollout shadow stage (`entelechy_release::run_shadow`) can rely on.
//!
//! Before this work the runtime had no shadow mode: `execute` always dispatched, and
//! "shadow commits no consequential effect" was only an accounting property of the
//! release layer, which trusted the harness to substitute effects.

use std::sync::{Arc, Mutex};

use entelechy_gateway::{MockModel, NativeToolGateway};
use entelechy_ir::{AuthorityEnvelope, LlmNode, Node, NodeKind, Program, ToolNode, Value};
use entelechy_runtime::{Budget, Engine, FailureReason, JournalEvent, RunStatus};

/// An authority envelope granting exactly `caps`.
fn auth(caps: &[&str]) -> AuthorityEnvelope {
    AuthorityEnvelope {
        capabilities: caps.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

/// A static tool node.
fn tool(id: &str, capability: &str) -> Node {
    Node::new(
        id,
        NodeKind::Tool(ToolNode {
            capability: capability.into(),
            args: serde_json::Value::Null,
        }),
    )
}

/// A `NativeToolGateway` whose `capability` closure increments a shared counter when
/// dispatched — a spy proving whether the engine actually called the tool.
fn spy_gateway(capability: &'static str) -> (NativeToolGateway, Arc<Mutex<u32>>) {
    let calls = Arc::new(Mutex::new(0u32));
    let mut tools = NativeToolGateway::new();
    {
        let calls = Arc::clone(&calls);
        tools.register(capability, move |_| {
            *calls.lock().unwrap() += 1;
            Ok(serde_json::json!({ "ok": true }))
        });
    }
    (tools, calls)
}

fn tool_effects(journal: &entelechy_runtime::Journal) -> usize {
    journal.tool_effect_count()
}

/// SEC/REP (headline). A consequential tool effect that a normal run dispatches is
/// NOT dispatched in shadow: the tool gateway is never called, the journal records no
/// `ToolEffect`, and the substitution log records what would have run — while the
/// *same* program run normally dispatches exactly once and journals a committed
/// effect. This is the before/after that defines "shadow is enforcement".
#[test]
fn shadow_intercepts_effect_that_execute_dispatches() {
    let model = MockModel::new();
    let prog = Program::new(auth(&["publish"]), tool("send", "publish"));

    // Shadow: zero dispatches, zero journaled effects, one recorded substitution.
    {
        let (mut tools, calls) = spy_gateway("publish");
        let mut e = Engine::new(&model, &mut tools);
        let sh = e.shadow(&prog, Value::trusted(serde_json::json!({})), "sh");

        assert_eq!(
            *calls.lock().unwrap(),
            0,
            "shadow must never call the tool gateway"
        );
        assert_eq!(sh.run.status, RunStatus::Succeeded, "{:?}", sh.run.status);
        assert_eq!(
            tool_effects(&sh.run.journal),
            0,
            "a shadow run's journal must carry no committed tool effect"
        );
        assert!(sh.committed_no_effects());
        assert_eq!(sh.substituted.len(), 1);
        assert_eq!(sh.substituted[0].capability, "publish");
        // The tool is the root node, so its recorded path is exactly "root".
        assert_eq!(sh.substituted[0].path, "root");
        // The record-only value flows downstream as null (the dry-run sink).
        assert_eq!(
            sh.run.output.as_ref().map(|v| &v.data),
            Some(&serde_json::Value::Null)
        );
    }

    // Execute (positive control): the same program dispatches exactly once and
    // journals a committed effect.
    {
        let (mut tools, calls) = spy_gateway("publish");
        let mut e = Engine::new(&model, &mut tools);
        let run = e.execute(&prog, Value::trusted(serde_json::json!({})), "ex");

        assert_eq!(
            *calls.lock().unwrap(),
            1,
            "execute must dispatch the effect"
        );
        assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);
        assert_eq!(
            tool_effects(&run.journal),
            1,
            "execute must journal the committed effect"
        );
    }
}

/// SEC (adversarial). Shadow is not a privileged bypass: a capability a `Delegate`
/// narrows away is still denied in shadow (WS2), and because the effect is denied the
/// tool gateway is never reached either. Effect substitution does not weaken the
/// authority gate.
#[test]
fn shadow_still_enforces_authority_narrowing() {
    let model = MockModel::new();
    let (mut tools, calls) = spy_gateway("refund");
    // Ambient grants refund; the delegate narrows it away, so the refund is denied.
    let prog = Program::new(
        auth(&["read_crm", "refund"]),
        Node::new(
            "root",
            NodeKind::Delegate {
                authority: auth(&["read_crm"]),
                body: Box::new(tool("refund", "refund")),
            },
        ),
    );
    let mut e = Engine::new(&model, &mut tools);
    let sh = e.shadow(&prog, Value::trusted(serde_json::json!({})), "sh-auth");

    assert!(
        matches!(
            sh.run.status,
            RunStatus::Failed {
                reason: FailureReason::AuthorityDenied(_),
                ..
            }
        ),
        "shadow must still deny a delegate-narrowed capability, got {:?}",
        sh.run.status
    );
    assert!(sh.run.output.is_none());
    // Denied before substitution — nothing was intercepted, nothing was dispatched.
    assert!(sh.substituted.is_empty());
    assert_eq!(*calls.lock().unwrap(), 0);
}

/// SEC. Shadow still charges and enforces the tool-call budget (WS3): a program with
/// more consequential effects than the budget allows halts `BudgetExhausted` in
/// shadow too, and still dispatches nothing. Substitution does not make effects free.
#[test]
fn shadow_still_enforces_budget() {
    let model = MockModel::new();
    let (mut tools, calls) = spy_gateway("publish");
    // Two tool nodes, but the budget allows only one tool call.
    let prog = Program::new(
        auth(&["publish"]),
        Node::new(
            "root",
            NodeKind::Seq(vec![tool("a", "publish"), tool("b", "publish")]),
        ),
    );
    let mut e = Engine::new(&model, &mut tools).with_budget(Budget {
        max_model_calls: 1_000,
        max_tool_calls: 1,
    });
    let sh = e.shadow(&prog, Value::trusted(serde_json::json!({})), "sh-budget");

    assert_eq!(
        sh.run.status,
        RunStatus::BudgetExhausted {
            budget: "max_tool_calls"
        },
        "shadow must enforce the tool-call budget, got {:?}",
        sh.run.status
    );
    assert_eq!(*calls.lock().unwrap(), 0, "shadow never dispatches");
    // The first effect was intercepted (within budget); the second tripped the budget
    // before it could be intercepted.
    assert_eq!(sh.substituted.len(), 1);
}

/// REP. Inference and the value flow still run live in shadow, so the shadow outcome
/// is comparable to the baseline's: a program that calls a model and then a tool
/// journals the real model call, substitutes the tool, and completes — the model call
/// is present in the journal while no tool effect is.
#[test]
fn shadow_runs_model_calls_live_but_substitutes_tools() {
    let model = MockModel::new();
    let (mut tools, calls) = spy_gateway("publish");
    let prog = Program::new(
        auth(&["publish"]),
        Node::new(
            "root",
            NodeKind::Seq(vec![
                Node::new(
                    "classify",
                    NodeKind::Llm(LlmNode {
                        model: "mock".into(),
                        prompt_template: "{input}".into(),
                        temperature: 0.0,
                    }),
                ),
                tool("send", "publish"),
            ]),
        ),
    );
    let mut e = Engine::new(&model, &mut tools);
    let sh = e.shadow(
        &prog,
        Value::trusted(serde_json::json!({ "x": 1 })),
        "sh-mix",
    );

    assert_eq!(sh.run.status, RunStatus::Succeeded, "{:?}", sh.run.status);
    assert_eq!(*calls.lock().unwrap(), 0, "the tool is substituted");
    // The model (inference) call ran live and is journaled…
    let model_calls = sh
        .run
        .journal
        .entries
        .iter()
        .filter(|entry| matches!(entry.event, JournalEvent::ModelCall { .. }))
        .count();
    assert_eq!(model_calls, 1, "the model call must run live in shadow");
    // …while the consequential tool effect is not.
    assert_eq!(tool_effects(&sh.run.journal), 0);
    assert_eq!(sh.substituted.len(), 1);
    assert_eq!(sh.substituted[0].capability, "publish");
    // Here the tool is a child of the Seq, so its recorded path names the node.
    assert!(
        sh.substituted[0].path.contains("send"),
        "substituted path should record the node: {}",
        sh.substituted[0].path
    );
}

/// REP (determinism). A shadow run is deterministic: the same program and input yield
/// the same substitution log and status every time (the whole discovery/replay model
/// relies on determinism, PRD 18).
#[test]
fn shadow_is_deterministic() {
    let model = MockModel::new();
    let prog = Program::new(
        auth(&["publish"]),
        Node::new(
            "root",
            NodeKind::Seq(vec![tool("a", "publish"), tool("b", "publish")]),
        ),
    );

    let run = |run_id: &str| {
        let (mut tools, _calls) = spy_gateway("publish");
        let mut e = Engine::new(&model, &mut tools);
        let sh = e.shadow(&prog, Value::trusted(serde_json::json!({})), run_id);
        (
            sh.run.status,
            sh.substituted
                .iter()
                .map(|s| s.capability.clone())
                .collect::<Vec<_>>(),
        )
    };

    let (status_a, caps_a) = run("sh-det-1");
    let (status_b, caps_b) = run("sh-det-2");
    assert_eq!(status_a, status_b);
    assert_eq!(caps_a, caps_b);
    assert_eq!(caps_a, vec!["publish".to_string(), "publish".to_string()]);
}
