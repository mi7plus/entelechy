//! The WP1 runtime-correctness enforcement drill (PRD 8, 5.4, 7.5, 8.5).
//!
//! A deterministic, self-contained demonstration that the runtime *enforces* the
//! WP1 invariants: a discovered architecture cannot launder information-flow
//! labels past a policy gate (WS1), exercise authority a `Delegate` removed (WS2),
//! bypass authorization on the counterfactual path (WS3), replay from the wrong
//! input (WS4), or complete with an effect whose commit outcome is unknown (WS5).
//!
//! It mirrors the operational drills (`slo`, `rollout`): it runs adversarial
//! programs on the real interpreter and reports each invariant SATISFIED/VIOLATED,
//! failing closed if any does not hold. This is the demonstrable artifact the
//! "verifiable runtime constraints" claim rests on — the red-team suite in
//! `tests/wp1_enforcement.rs` pins the same invariants in CI.

use std::collections::HashMap;

use entelechy_gateway::{MockModel, NativeToolGateway};
use entelechy_ir::{
    AttestationLevel, AuthorityEnvelope, CodeNode, EffectClass, EffectMetadata, LlmNode, Node,
    NodeKind, Program, ToolNode, Value,
};
use entelechy_policy::{NativePolicy, PolicySnapshot};
use serde::{Deserialize, Serialize};

use crate::{CommitStatus, Engine, FailureReason, JournalEvent, PolicyConfig, RunStatus};

/// The outcome of the WP1 enforcement drill: one boolean per enforced invariant,
/// grouped by workstream, plus the aggregate `ok`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnforcementDrillReport {
    /// WS1 (SEC): a `Map` over a tainted collection cannot launder the taint past a
    /// policy gate into a consequential egress — the egress is denied.
    pub map_cannot_launder_taint: bool,
    /// WS1 (SEC): a fully-trusted fan-out is not over-tainted — the positive control
    /// that the same egress is authorized for a trusted collection.
    pub trusted_fanout_not_overtainted: bool,
    /// WS2 (SEC): a capability a `Delegate` dropped is denied inside its subtree,
    /// even though the ambient authority grants it.
    pub delegate_narrows_authority: bool,
    /// WS2 (SEC): a `Delegate` whose envelope widens the active one halts at
    /// runtime, before any body effect runs.
    pub delegate_widening_halts: bool,
    /// WS3 (SEC): a live counterfactual tool effect is authorized exactly as in
    /// `Execute` — a forbidden capability is denied on the post-divergence path.
    pub counterfactual_effect_authorized: bool,
    /// WS4 (REP): replay reconstructs from the recorded initial input, reproducing
    /// the real output rather than the output of an empty object.
    pub replay_uses_recorded_input: bool,
    /// WS4 (REP): replaying a different input than the recorded one is detected and
    /// rejected rather than silently run.
    pub replay_detects_input_tampering: bool,
    /// WS5 (REP): a dispatched effect whose acknowledgement is lost leaves a durable
    /// write-ahead intent, forcing `ReconciliationRequired` (never a silent success
    /// and never a blind retry).
    pub unacknowledged_effect_reconciles: bool,
    /// WS5 (REP): an acknowledged effect resolves its write-ahead intent to
    /// `Committed`, leaving no dangling intent and replaying cleanly.
    pub acknowledged_effect_commits: bool,
    /// Whether every WP1 invariant above held.
    pub ok: bool,
}

/// Run the WP1 runtime-correctness enforcement drill. Deterministic and
/// side-effect free: it builds adversarial programs and runs them on fresh
/// engines, returning the per-invariant verdicts.
pub fn enforcement_drill() -> EnforcementDrillReport {
    let (map_cannot_launder_taint, trusted_fanout_not_overtainted) = probe_ws1_ifc();
    let (delegate_narrows_authority, delegate_widening_halts) = probe_ws2_authority();
    let counterfactual_effect_authorized = probe_ws3_parity();
    let (replay_uses_recorded_input, replay_detects_input_tampering) = probe_ws4_replay();
    let (unacknowledged_effect_reconciles, acknowledged_effect_commits) = probe_ws5_writeahead();

    let ok = map_cannot_launder_taint
        && trusted_fanout_not_overtainted
        && delegate_narrows_authority
        && delegate_widening_halts
        && counterfactual_effect_authorized
        && replay_uses_recorded_input
        && replay_detects_input_tampering
        && unacknowledged_effect_reconciles
        && acknowledged_effect_commits;

    EnforcementDrillReport {
        map_cannot_launder_taint,
        trusted_fanout_not_overtainted,
        delegate_narrows_authority,
        delegate_widening_halts,
        counterfactual_effect_authorized,
        replay_uses_recorded_input,
        replay_detects_input_tampering,
        unacknowledged_effect_reconciles,
        acknowledged_effect_commits,
        ok,
    }
}

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

/// A consequential (Write, no approval obligation) effect metadata entry.
fn consequential(namespace: &str) -> EffectMetadata {
    EffectMetadata {
        class: EffectClass::Write,
        idempotent: false,
        reversible: true,
        dry_run_supported: false,
        read_back_supported: false,
        attestation: AttestationLevel::OperatorAttested,
        operation_key_namespace: namespace.into(),
    }
}

/// WS1 — a `Map` must not launder a tainted collection into a consequential egress,
/// and a trusted collection must still be authorized (no over-tainting).
fn probe_ws1_ifc() -> (bool, bool) {
    let prog = |authority: AuthorityEnvelope| {
        Program::new(
            authority,
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "fanout",
                        NodeKind::Map {
                            over: "items".into(),
                            body: Box::new(Node::new(
                                "copy",
                                NodeKind::Code(CodeNode {
                                    function: "echo".into(),
                                }),
                            )),
                        },
                    ),
                    tool("send", "publish"),
                ]),
            ),
        )
    };
    let mut authority = AuthorityEnvelope::empty();
    authority.capabilities.insert("publish".into());
    let policy = |authority: &AuthorityEnvelope| {
        let mut catalog = HashMap::new();
        catalog.insert("publish".to_string(), consequential("pub"));
        PolicyConfig {
            engine: Box::new(NativePolicy::new()),
            authority: authority.clone(),
            snapshot: PolicySnapshot::default(),
            catalog,
            principal: "runtime".into(),
        }
    };

    let model = MockModel::new();

    // Tainted collection: the Map-laundered egress must be denied.
    let tainted_denied = {
        let mut tools = NativeToolGateway::new();
        tools.register("publish", |_| Ok(serde_json::json!({ "published": true })));
        let mut e = Engine::new(&model, &mut tools).with_policy(policy(&authority));
        e.register_code("echo", |v| Ok(v.data.clone()));
        let run = e.execute(
            &prog(authority.clone()),
            Value::tainted(serde_json::json!({ "items": [1] })),
            "drill-ws1-attack",
        );
        matches!(
            run.status,
            RunStatus::Failed {
                reason: FailureReason::PolicyDenied(_),
                ..
            }
        ) && run.output.is_none()
    };

    // Trusted collection: the same egress is authorized (positive control).
    let trusted_ok = {
        let mut tools = NativeToolGateway::new();
        tools.register("publish", |_| Ok(serde_json::json!({ "published": true })));
        let mut e = Engine::new(&model, &mut tools).with_policy(policy(&authority));
        e.register_code("echo", |v| Ok(v.data.clone()));
        let run = e.execute(
            &prog(authority.clone()),
            Value::trusted(serde_json::json!({ "items": [1] })),
            "drill-ws1-control",
        );
        run.status == RunStatus::Succeeded
    };

    (tainted_denied, trusted_ok)
}

/// WS2 — a `Delegate` narrows authority (a dropped capability is denied inside the
/// subtree) and a widening `Delegate` halts at runtime.
fn probe_ws2_authority() -> (bool, bool) {
    let model = MockModel::new();

    let narrows = {
        let mut tools = NativeToolGateway::new();
        tools.register("refund", |_| Ok(serde_json::json!({ "refunded": true })));
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
        let run = e.execute(
            &prog,
            Value::trusted(serde_json::json!({})),
            "drill-ws2-narrow",
        );
        matches!(
            run.status,
            RunStatus::Failed {
                reason: FailureReason::AuthorityDenied(_),
                ..
            }
        ) && run.output.is_none()
    };

    let widening_halts = {
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
        let run = e.execute(
            &prog,
            Value::trusted(serde_json::json!({})),
            "drill-ws2-widen",
        );
        matches!(
            run.status,
            RunStatus::Failed {
                reason: FailureReason::AuthorityDenied(_),
                ..
            }
        ) && run.journal.entries.is_empty()
    };

    (narrows, widening_halts)
}

/// WS3 — a live counterfactual tool effect passes the same authorization gate as
/// `Execute`: a forbidden capability is denied on the post-divergence path.
fn probe_ws3_parity() -> bool {
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    tools.register("refund", |_| Ok(serde_json::json!({ "refunded": true })));

    let mut authority = AuthorityEnvelope::empty();
    authority.forbidden_capabilities.insert("refund".into());

    let prog = Program::new(
        authority.clone(),
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
                tool("refund", "refund"),
            ]),
        ),
    );

    let make_policy = || {
        let mut catalog = HashMap::new();
        catalog.insert("refund".to_string(), {
            let mut m = consequential("pay");
            m.class = EffectClass::Irreversible;
            m.reversible = false;
            m
        });
        PolicyConfig {
            engine: Box::new(NativePolicy::new()),
            authority: authority.clone(),
            snapshot: PolicySnapshot::default(),
            catalog,
            principal: "runtime".into(),
        }
    };

    // Record a journal: the Execute run is denied at the forbidden refund, but the
    // classify model call is journaled first (so there is a pre-divergence span).
    let journal = {
        let mut e = Engine::new(&model, &mut tools).with_policy(make_policy());
        let run = e.execute(
            &prog,
            Value::trusted(serde_json::json!({})),
            "drill-ws3-rec",
        );
        if !matches!(
            run.status,
            RunStatus::Failed {
                reason: FailureReason::PolicyDenied(_),
                ..
            }
        ) {
            return false;
        }
        run.journal
    };

    // Counterfactual: intervene at classify, re-execute the refund tool live. It
    // must be denied exactly as in Execute.
    let mut e_cf = Engine::new(&model, &mut tools).with_policy(make_policy());
    let cf = e_cf.counterfactual(
        &prog,
        &journal,
        "root/0:classify",
        Value::tainted(serde_json::json!({ "text": "hi" })),
    );
    cf.diverged && cf.output.is_none() && e_cf.policy_decisions().any_denied()
}

/// WS4 — replay reconstructs from the recorded input, and a tampered input is
/// detected.
fn probe_ws4_replay() -> (bool, bool) {
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    let mut e = Engine::new(&model, &mut tools);
    e.register_code("double", |v| {
        let n = v.data.get("n").and_then(|n| n.as_i64()).unwrap_or(0);
        Ok(serde_json::json!({ "doubled": n * 2 }))
    });
    let prog = Program::new(
        AuthorityEnvelope::empty(),
        Node::new(
            "calc",
            NodeKind::Code(CodeNode {
                function: "double".into(),
            }),
        ),
    );

    let input = Value::trusted(serde_json::json!({ "n": 21 }));
    let run = e.execute(&prog, input.clone(), "drill-ws4");
    let recorded_ok = run.status == RunStatus::Succeeded
        && run
            .output
            .as_ref()
            .map(|o| o.data == serde_json::json!({ "doubled": 42 }))
            .unwrap_or(false)
        && e.replay(&prog, input, &run.journal)
            .ok()
            .flatten()
            .map(|v| v.data == serde_json::json!({ "doubled": 42 }))
            .unwrap_or(false);

    let tamper_detected = matches!(
        e.replay(
            &prog,
            Value::trusted(serde_json::json!({ "n": 999 })),
            &run.journal,
        ),
        Err(crate::ReplayMismatch::InputMismatch)
    );

    (recorded_ok, tamper_detected)
}

/// WS5 — an unacknowledged dispatch leaves a write-ahead intent that forces
/// reconciliation; an acknowledged dispatch resolves it to `Committed`.
fn probe_ws5_writeahead() -> (bool, bool) {
    let model = MockModel::new();

    let unacknowledged_reconciles = {
        let mut tools = NativeToolGateway::new();
        tools.register("send_payment", |_| Err("acknowledgement lost".into()));
        let prog = Program::new(auth(&["send_payment"]), tool("pay", "send_payment"));
        let mut e = Engine::new(&model, &mut tools);
        let run = e.execute(
            &prog,
            Value::trusted(serde_json::json!({ "amount": 1 })),
            "drill-ws5-crash",
        );
        run.status == RunStatus::ReconciliationRequired
            && run.output.is_none()
            && run.journal.has_unknown_commit()
    };

    let acknowledged_commits = {
        let mut tools = NativeToolGateway::new();
        tools.register("publish", |_| Ok(serde_json::json!({ "id": 7 })));
        let prog = Program::new(auth(&["publish"]), tool("send", "publish"));
        let mut e = Engine::new(&model, &mut tools);
        let run = e.execute(&prog, Value::trusted(serde_json::json!({})), "drill-ws5-ok");
        let committed = run
            .journal
            .entries
            .first()
            .map(|entry| {
                matches!(
                    &entry.event,
                    JournalEvent::ToolEffect {
                        commit: CommitStatus::Committed,
                        ..
                    }
                )
            })
            .unwrap_or(false);
        run.status == RunStatus::Succeeded && !run.journal.has_unknown_commit() && committed
    };

    (unacknowledged_reconciles, acknowledged_commits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforcement_drill_reports_all_invariants_satisfied() {
        let report = enforcement_drill();
        assert!(
            report.ok,
            "WP1 enforcement drill must report every invariant SATISFIED: {report:?}"
        );
    }
}
