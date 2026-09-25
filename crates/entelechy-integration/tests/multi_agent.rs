//! Multi-agent design pipeline tests (PRD 11.4 level 4/5, 8.2).
//!
//! Exercises the whole multi-agent path across crates: synthesize a multi-agent
//! design (design), confirm it is structurally valid including authority narrowing
//! (IR-I6), transform a single agent into a parallel committee via the level-4
//! operator, then execute the result on the interpreter and replay it
//! deterministically (runtime).

use std::collections::HashMap;

use entelechy_design::{apply, synthesize_multi_agent, AgentRole, AgentSpec, EditOp};
use entelechy_gateway::{MockModel, NativeToolGateway};
use entelechy_ir::{AuthorityEnvelope, EffectMetadata, NodeKind, Value};
use entelechy_runtime::{Engine, RunStatus};

fn empty_catalog() -> HashMap<String, EffectMetadata> {
    HashMap::new()
}

/// A synthesized multi-agent design is valid and runs end to end.
#[test]
fn synthesized_multi_agent_validates_executes_and_replays() {
    let mut coord = AuthorityEnvelope::empty();
    coord.capabilities.insert("read_crm".into());
    coord.capabilities.insert("summarize".into());
    let mut researcher_auth = AuthorityEnvelope::empty();
    researcher_auth.capabilities.insert("read_crm".into());
    let mut writer_auth = AuthorityEnvelope::empty();
    writer_auth.capabilities.insert("summarize".into());

    let program = synthesize_multi_agent(
        coord,
        &[
            AgentRole {
                id: "researcher".into(),
                model: "mock".into(),
                prompt_template: "research: {input}".into(),
                authority: researcher_auth,
            },
            AgentRole {
                id: "writer".into(),
                model: "mock".into(),
                prompt_template: "write: {input}".into(),
                authority: writer_auth,
            },
        ],
    );

    // Structurally valid: each Delegate narrows the coordinator authority (IR-I6).
    let violations = entelechy_ir::validate(&program, &empty_catalog());
    assert!(
        violations.is_empty(),
        "multi-agent design should be valid, got {violations:?}"
    );
    assert!(matches!(program.root.kind, NodeKind::Par(_)));

    // Executes: both delegated sub-agents run, and the parallel result is an array
    // of their two outputs (Par semantics, RK-7).
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    let mut engine = Engine::new(&model, &mut tools);
    let input = Value::trusted(serde_json::json!({ "ticket": "help" }));
    let run = engine.execute(&program, input.clone(), "ma-run");
    assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);
    assert_eq!(run.journal.entries.len(), 2, "two sub-agent model calls");
    assert!(run.output.as_ref().unwrap().data.is_array());

    // Replays deterministically against its own journal (PRD 8.2).
    assert!(engine
        .replay(&program, input, &run.journal)
        .unwrap()
        .is_some());
}

/// The level-4 SplitParallel operator turns a single agent into a committee that
/// stays valid and executable.
#[test]
fn split_parallel_operator_produces_a_runnable_committee() {
    let program = entelechy_design::synthesize_single_agent(
        AuthorityEnvelope::empty(),
        "mock",
        "resolve: {input}",
    );
    // The single-agent baseline's Llm node is "agent".
    let committee = apply(
        &program,
        &EditOp::SplitParallel {
            node: "agent".into(),
            agents: vec![
                AgentSpec {
                    id: "optimist".into(),
                    prompt_template: "best case: {input}".into(),
                },
                AgentSpec {
                    id: "skeptic".into(),
                    prompt_template: "worst case: {input}".into(),
                },
            ],
            reducer: AgentSpec {
                id: "coordinator".into(),
                prompt_template: "synthesize the results into one answer: {input}".into(),
            },
        },
    )
    .expect("split should apply to the baseline agent");

    assert!(entelechy_ir::validate(&committee, &empty_catalog()).is_empty());

    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    let mut engine = Engine::new(&model, &mut tools);
    let run = engine.execute(
        &committee,
        Value::trusted(serde_json::json!({ "q": "x" })),
        "committee-run",
    );
    assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);
    // Two committee members plus the coordinator each made one model call, and the
    // final output is the coordinator's single synthesized value (not a raw array).
    assert_eq!(run.journal.entries.len(), 3);
    assert!(!run.output.as_ref().unwrap().data.is_array());
}
