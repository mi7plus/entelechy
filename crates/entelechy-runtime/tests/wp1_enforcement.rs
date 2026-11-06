//! WP1 runtime-correctness red-team suite (PRD 8, 5.4).
//!
//! Adversarial, public-API probes that a *discovered* architecture cannot violate
//! the runtime's authority or recoverability constraints no matter what the search
//! emits.
//!
//! WS2 — dynamic authority narrowing at `Delegate` (A2 / IR-I6): the runtime
//! maintains an authority stack seeded from `program.authority` and narrowed by
//! every enclosing `Delegate`, and every consequential tool effect inside a
//! delegated scope is authorized against the active (narrowed) envelope — enforced
//! at runtime, independent of any policy configuration. Before WS2 the runtime
//! ignored `program.authority` entirely and `Delegate` dropped its authority field
//! and ran the body unchanged.
//!
//! WS5 — write-ahead intent + reconciliation (PRD 7.6, 8.5, 8.6): a consequential
//! tool effect records a durable intent (`commit = Unknown`) *before* dispatch and
//! resolves it to `Committed` only on acknowledgement; an unacknowledged dispatch
//! leaves a dangling Unknown effect, forcing `ReconciliationRequired`. Before WS5
//! the effect was journaled as `Committed` only *after* the call returned, so a
//! crash between dispatch and journaling left no durable trace.

use entelechy_gateway::{MockModel, NativeToolGateway};
use entelechy_ir::{
    AuthorityEnvelope, CodeNode, Condition, Node, NodeKind, Program, ToolNode, Value,
};
use entelechy_runtime::{CommitStatus, Engine, FailureReason, JournalEvent, RunStatus};

/// An authority envelope granting exactly `caps` (no forbids, scopes, or limits).
fn auth(caps: &[&str]) -> AuthorityEnvelope {
    AuthorityEnvelope {
        capabilities: caps.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn tool(id: &str, capability: &str) -> Node {
    Node::new(
        id,
        NodeKind::Tool(ToolNode {
            capability: capability.into(),
            args: serde_json::Value::Null,
        }),
    )
}

/// WS2 · SEC. A `Delegate` that drops a capability denies it inside the subtree,
/// even though the ambient authority grants it — while a capability the delegate
/// retains still runs. This is the core narrowing invariant.
#[test]
fn delegate_narrows_active_authority() {
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    tools.register("read_crm", |_| Ok(serde_json::json!({ "crm": "ok" })));
    tools.register("refund", |_| Ok(serde_json::json!({ "refunded": true })));

    // Ambient grants read_crm AND refund; the delegate narrows to {read_crm}.
    let prog = Program::new(
        auth(&["read_crm", "refund"]),
        Node::new(
            "root",
            NodeKind::Delegate {
                authority: auth(&["read_crm"]),
                body: Box::new(Node::new(
                    "body",
                    // read_crm is retained (runs); refund was narrowed away (denied).
                    NodeKind::Seq(vec![tool("read", "read_crm"), tool("refund", "refund")]),
                )),
            },
        ),
    );

    let mut e = Engine::new(&model, &mut tools);
    let run = e.execute(&prog, Value::trusted(serde_json::json!({})), "d-narrow");

    match run.status {
        RunStatus::Failed {
            node_path,
            reason: FailureReason::AuthorityDenied(_),
        } => assert!(
            node_path.contains("refund"),
            "expected denial at the refund node, got {node_path}"
        ),
        other => panic!("expected AuthorityDenied at refund inside the delegate, got {other:?}"),
    }
    assert!(run.output.is_none());
    // read_crm (retained) ran and journaled its effect; refund never dispatched.
    assert_eq!(
        run.journal.entries.len(),
        1,
        "the retained capability must run while the narrowed one is denied"
    );
}

/// WS2 · SEC. A `Delegate` whose envelope is not a subset of the active envelope
/// (it widens authority) halts at the delegate node at runtime — defense-in-depth
/// beyond the static IR-I6 check — before any body effect runs. A narrowing
/// (subset) delegate does not halt.
#[test]
fn delegate_rejects_runtime_widening() {
    let model = MockModel::new();

    // Widening: root grants {read_crm}; the delegate tries to add refund.
    {
        let mut tools = NativeToolGateway::new();
        tools.register("refund", |_| Ok(serde_json::json!({ "refunded": true })));
        let prog = Program::new(
            auth(&["read_crm"]),
            Node::new(
                "root",
                NodeKind::Delegate {
                    authority: auth(&["read_crm", "refund"]),
                    body: Box::new(tool("refund", "refund")),
                },
            ),
        );
        let mut e = Engine::new(&model, &mut tools);
        let run = e.execute(&prog, Value::trusted(serde_json::json!({})), "d-widen");
        assert!(
            matches!(
                run.status,
                RunStatus::Failed {
                    reason: FailureReason::AuthorityDenied(_),
                    ..
                }
            ),
            "a widening delegate must halt at runtime, got {:?}",
            run.status
        );
        assert!(run.output.is_none());
        // The body never dispatched the refund (the halt is at the delegate itself).
        assert!(run.journal.entries.is_empty());
    }

    // Control: a narrowing (subset) delegate runs its body normally.
    {
        let mut tools = NativeToolGateway::new();
        let prog = Program::new(
            auth(&["read_crm"]),
            Node::new(
                "root",
                NodeKind::Delegate {
                    authority: AuthorityEnvelope::empty(), // {} ⊆ {read_crm}
                    body: Box::new(Node::new(
                        "noop",
                        NodeKind::Code(CodeNode {
                            function: "echo".into(),
                        }),
                    )),
                },
            ),
        );
        let mut e = Engine::new(&model, &mut tools);
        e.register_code("echo", |v| Ok(v.data.clone()));
        let run = e.execute(
            &prog,
            Value::trusted(serde_json::json!({ "x": 1 })),
            "d-sub",
        );
        assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);
    }
}

/// WS2 · SEC. Nested delegates narrow monotonically: a capability dropped by an
/// inner delegate cannot be regained by anything inside it, and an inner delegate
/// that widens relative to its immediate parent halts.
#[test]
fn nested_delegates_narrow_monotonically() {
    let model = MockModel::new();

    // root {a,b,c} > d1 {a,b} > d2 {a}. Inside d2, Tool(a) runs but Tool(b) — which
    // d2 dropped — is denied, even though b is still in d1 and in the root.
    {
        let mut tools = NativeToolGateway::new();
        tools.register("cap_a", |_| Ok(serde_json::json!({ "a": true })));
        tools.register("cap_b", |_| Ok(serde_json::json!({ "b": true })));
        let prog = Program::new(
            auth(&["cap_a", "cap_b", "cap_c"]),
            Node::new(
                "d1",
                NodeKind::Delegate {
                    authority: auth(&["cap_a", "cap_b"]),
                    body: Box::new(Node::new(
                        "d2",
                        NodeKind::Delegate {
                            authority: auth(&["cap_a"]),
                            body: Box::new(Node::new(
                                "inner",
                                NodeKind::Seq(vec![tool("a", "cap_a"), tool("b", "cap_b")]),
                            )),
                        },
                    )),
                },
            ),
        );
        let mut e = Engine::new(&model, &mut tools);
        let run = e.execute(&prog, Value::trusted(serde_json::json!({})), "nest-narrow");
        match run.status {
            RunStatus::Failed {
                node_path,
                reason: FailureReason::AuthorityDenied(_),
            } => assert!(
                node_path.contains(":b"),
                "expected denial at b, got {node_path}"
            ),
            other => panic!("expected cap_b denied in the innermost delegate, got {other:?}"),
        }
        // cap_a (still granted innermost) ran; cap_b never dispatched.
        assert_eq!(run.journal.entries.len(), 1);
    }

    // An inner delegate that widens relative to its immediate parent halts:
    // d1 {a,b}, then d2 {a,c} — c is in the root but not in d1, so d2 ⊄ d1.
    {
        let mut tools = NativeToolGateway::new();
        let prog = Program::new(
            auth(&["cap_a", "cap_b", "cap_c"]),
            Node::new(
                "d1",
                NodeKind::Delegate {
                    authority: auth(&["cap_a", "cap_b"]),
                    body: Box::new(Node::new(
                        "d2",
                        NodeKind::Delegate {
                            authority: auth(&["cap_a", "cap_c"]),
                            body: Box::new(Node::new(
                                "noop",
                                NodeKind::Code(CodeNode {
                                    function: "echo".into(),
                                }),
                            )),
                        },
                    )),
                },
            ),
        );
        let mut e = Engine::new(&model, &mut tools);
        e.register_code("echo", |v| Ok(v.data.clone()));
        let run = e.execute(&prog, Value::trusted(serde_json::json!({})), "nest-widen");
        assert!(
            matches!(
                run.status,
                RunStatus::Failed {
                    reason: FailureReason::AuthorityDenied(_),
                    ..
                }
            ),
            "a nested widening relative to the immediate parent must halt, got {:?}",
            run.status
        );
    }
}

/// WS2 · SEC (adversarial). A data-dependent control path cannot exercise a
/// capability outside the active delegated envelope. The IR's `Tool.capability` is
/// static, so "a capability that comes from input data" is modelled as a `Branch`
/// on an input field selecting a privileged `Tool` — exactly the escape a search
/// could emit. The privileged capability is granted by the ambient authority but
/// not by the delegate, so no input can route around the narrowing.
#[test]
fn data_dependent_capability_cannot_escape_delegate() {
    let model = MockModel::new();

    // Delegate narrowed to {classify}; `escalate` is ambient-granted but not inside.
    let program = |then_cap: &str| {
        Program::new(
            auth(&["classify", "escalate"]),
            Node::new(
                "root",
                NodeKind::Delegate {
                    authority: auth(&["classify"]),
                    body: Box::new(Node::new(
                        "route",
                        NodeKind::Branch {
                            cond: Condition::Truthy {
                                field: "escalate".into(),
                            },
                            then: Box::new(tool("priv", then_cap)),
                            els: Box::new(Node::new(
                                "safe",
                                NodeKind::Code(CodeNode {
                                    function: "echo".into(),
                                }),
                            )),
                        },
                    )),
                },
            ),
        )
    };

    // The data selects the privileged branch -> escalate -> denied by the envelope.
    {
        let mut tools = NativeToolGateway::new();
        tools.register("escalate", |_| Ok(serde_json::json!({ "escalated": true })));
        let mut e = Engine::new(&model, &mut tools);
        e.register_code("echo", |v| Ok(v.data.clone()));
        let run = e.execute(
            &program("escalate"),
            Value::tainted(serde_json::json!({ "escalate": true })),
            "esc-attack",
        );
        assert!(
            matches!(
                run.status,
                RunStatus::Failed {
                    reason: FailureReason::AuthorityDenied(_),
                    ..
                }
            ),
            "the data-selected privileged capability must be denied, got {:?}",
            run.status
        );
        assert!(run.output.is_none());
    }

    // Control: the safe branch (within the envelope) runs regardless.
    {
        let mut tools = NativeToolGateway::new();
        let mut e = Engine::new(&model, &mut tools);
        e.register_code("echo", |v| Ok(v.data.clone()));
        let run = e.execute(
            &program("escalate"),
            Value::tainted(serde_json::json!({ "escalate": false })),
            "esc-safe",
        );
        assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);
    }
}

/// WS2 · regression. A program with no `Delegate` is at stack depth 1, so the
/// active envelope is exactly `program.authority` and WS2 adds no top-level
/// enforcement: a tool-bearing program runs exactly as it did before WS2 (top-level
/// authority stays the policy layer's concern). This pins "no-Delegate programs
/// behave identically".
#[test]
fn delegate_free_programs_behave_identically() {
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    tools.register("publish", |_| Ok(serde_json::json!({ "published": true })));

    // Empty authority, no policy, a bare Tool: unchanged from pre-WS2, it runs.
    let prog = Program::new(AuthorityEnvelope::empty(), tool("send", "publish"));
    let mut e = Engine::new(&model, &mut tools);
    let run = e.execute(&prog, Value::trusted(serde_json::json!({})), "no-delegate");
    assert_eq!(
        run.status,
        RunStatus::Succeeded,
        "a no-delegate program must behave identically to pre-WS2, got {:?}",
        run.status
    );
    assert_eq!(run.journal.entries.len(), 1);
}

/// WS5 · REP. A tool whose dispatch is not acknowledged still leaves a durable
/// write-ahead intent (`commit = Unknown`) in the journal — proving the intent is
/// recorded *before* the effect, not after. Before WS5 a failed tool left no
/// journal entry at all.
#[test]
fn tool_intent_is_journaled_before_dispatch() {
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    tools.register("charge_card", |_| Err("acknowledgement lost".into()));

    let prog = Program::new(auth(&["charge_card"]), tool("charge", "charge_card"));
    let mut e = Engine::new(&model, &mut tools);
    let run = e.execute(&prog, Value::trusted(serde_json::json!({})), "wal-intent");

    // A dangling intent means the run cannot be declared successful.
    assert_eq!(
        run.status,
        RunStatus::ReconciliationRequired,
        "{:?}",
        run.status
    );
    assert!(
        run.output.is_none(),
        "outputs are withheld pending reconciliation"
    );
    // Exactly one ToolEffect intent is present, still Unknown (unresolved).
    assert_eq!(run.journal.entries.len(), 1);
    match &run.journal.entries[0].event {
        JournalEvent::ToolEffect {
            capability, commit, ..
        } => {
            assert_eq!(capability, "charge_card");
            assert_eq!(
                *commit,
                CommitStatus::Unknown,
                "the intent must be unresolved"
            );
        }
        other => panic!("expected a ToolEffect intent, got {other:?}"),
    }
    assert!(run.journal.has_unknown_commit());
}

/// WS5 · REP (adversarial). A crash between dispatch and acknowledgement: the tool
/// performs its external side effect and then the acknowledgement is lost. The
/// write-ahead intent stays Unknown, so the run reports `ReconciliationRequired` —
/// the effect may have taken hold and must be reconciled, never blindly retried or
/// silently dropped (PRD 8.5/8.6).
#[test]
fn crash_between_dispatch_and_commit_is_recoverable() {
    use std::sync::{Arc, Mutex};

    let dispatched = Arc::new(Mutex::new(0u32));
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    {
        let dispatched = Arc::clone(&dispatched);
        tools.register("send_payment", move |_| {
            *dispatched.lock().unwrap() += 1; // the external effect happened…
            Err("crash before acknowledgement".into()) // …but the ack never arrived
        });
    }

    let prog = Program::new(auth(&["send_payment"]), tool("pay", "send_payment"));
    let mut e = Engine::new(&model, &mut tools);
    let run = e.execute(
        &prog,
        Value::trusted(serde_json::json!({ "amount": 10 })),
        "wal-crash",
    );

    assert_eq!(
        run.status,
        RunStatus::ReconciliationRequired,
        "{:?}",
        run.status
    );
    assert!(run.output.is_none());
    // The effect was dispatched exactly once — recovery never blindly re-dispatches.
    assert_eq!(*dispatched.lock().unwrap(), 1);
    // The journal records the dangling intent for reconciliation.
    assert!(run.journal.has_unknown_commit());
}

/// WS5 · REP. The happy path: an acknowledged dispatch resolves the write-ahead
/// intent to `Committed` with the real response, leaving exactly one committed
/// effect per tool node — replay-stable and byte-identical to pre-WS5.
#[test]
fn tool_intent_resolves_to_committed_on_acknowledgement() {
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    tools.register("publish", |_| Ok(serde_json::json!({ "id": 7 })));

    let prog = Program::new(auth(&["publish"]), tool("send", "publish"));
    let mut e = Engine::new(&model, &mut tools);
    let run = e.execute(&prog, Value::trusted(serde_json::json!({})), "wal-ok");

    assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);
    assert!(
        !run.journal.has_unknown_commit(),
        "a committed effect is not dangling"
    );
    assert_eq!(run.journal.entries.len(), 1);
    match &run.journal.entries[0].event {
        JournalEvent::ToolEffect {
            commit, response, ..
        } => {
            assert_eq!(*commit, CommitStatus::Committed);
            assert_eq!(response, &serde_json::json!({ "id": 7 }));
        }
        other => panic!("expected a committed ToolEffect, got {other:?}"),
    }
    // Replay reproduces the committed effect with no divergence.
    let replayed = e
        .replay(&prog, Value::trusted(serde_json::json!({})), &run.journal)
        .expect("replay must not diverge");
    assert!(replayed.is_some());
}

// --- Tool-effect failure taxonomy (PRD 8.5/8.6) ------------------------------
//
// A *failed* tool call splits by whether a side effect could remain. A definitive
// rejection that committed nothing fails cleanly (`Failed` + `ToolError`, no
// dangling intent); only a lost/indeterminate acknowledgement forces
// `ReconciliationRequired`. The two WS5 tests above (`charge_card`/`send_payment`
// via plain `register`, whose `Err` is `Indeterminate`) remain the reconciliation
// regression guard; these two cover the clean-failure side.

/// TAXONOMY · REP. An unregistered capability is an `UnknownCapability` error, which
/// committed nothing: the run fails cleanly with a `ToolError` and leaves *no*
/// dangling intent — it must not be mistaken for a lost acknowledgement that forces
/// reconciliation. (The authority grants the capability, so dispatch is reached.)
#[test]
fn unknown_capability_fails_cleanly_not_reconciliation() {
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new(); // `missing_tool` is never registered

    let prog = Program::new(auth(&["missing_tool"]), tool("x", "missing_tool"));
    let mut e = Engine::new(&model, &mut tools);
    let run = e.execute(&prog, Value::trusted(serde_json::json!({})), "unknown-cap");

    match &run.status {
        RunStatus::Failed {
            reason: FailureReason::ToolError(_),
            ..
        } => {}
        other => panic!("expected a clean ToolError failure, got {other:?}"),
    }
    assert!(
        !run.journal.has_unknown_commit(),
        "a call that committed nothing leaves no intent to reconcile"
    );
    assert!(run.output.is_none(), "a failed run yields no output");
}

/// TAXONOMY · REP. A *guarded* tool validates the call and refuses it before any
/// side effect, so its `Err` is a definitive rejection (`NoEffect`): the run fails
/// cleanly, dropping the speculative write-ahead intent rather than leaving it
/// dangling for reconciliation.
#[test]
fn guarded_rejection_fails_cleanly() {
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    tools.register_guarded("validate_amount", |_| Err("amount out of range".into()));

    let prog = Program::new(auth(&["validate_amount"]), tool("check", "validate_amount"));
    let mut e = Engine::new(&model, &mut tools);
    let run = e.execute(
        &prog,
        Value::trusted(serde_json::json!({})),
        "guarded-reject",
    );

    match &run.status {
        RunStatus::Failed {
            reason: FailureReason::ToolError(_),
            ..
        } => {}
        other => panic!("expected a clean ToolError failure, got {other:?}"),
    }
    assert!(
        !run.journal.has_unknown_commit(),
        "a guarded rejection commits nothing, so no intent dangles"
    );
    assert!(run.output.is_none());
}
