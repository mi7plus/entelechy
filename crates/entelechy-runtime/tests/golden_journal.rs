//! Golden format-stability test for the execution journal (PRD 16.5/16.6, 8.2).
//!
//! The journal wire format is a compatibility promise: a recorded journal must
//! stay replayable across versions. This pins the canonical JSON of a journal
//! from a fixed, deterministic run (the mock model is a pure function of its
//! request), so a serialization change — a renamed field, a changed `kind` tag,
//! reordered structure — fails CI instead of silently breaking replay of stored
//! journals.
//!
//! If this fails because the format changed on purpose, update the golden value
//! and note the journal-format change in CHANGELOG.md.

use entelechy_artifacts::to_canonical_bytes;
use entelechy_gateway::{MockModel, NativeToolGateway};
use entelechy_ir::{AuthorityEnvelope, LlmNode, Node, NodeKind, Program, Value};
use entelechy_runtime::{Engine, RunStatus};

fn single_llm_program() -> Program {
    Program::new(
        AuthorityEnvelope::empty(),
        Node::new(
            "agent",
            NodeKind::Llm(LlmNode {
                model: "mock".into(),
                prompt_template: "answer: {input}".into(),
                temperature: 0.0,
            }),
        ),
    )
}

#[test]
fn journal_wire_format_is_stable() {
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    let mut engine = Engine::new(&model, &mut tools);
    let run = engine.execute(
        &single_llm_program(),
        Value::trusted(serde_json::json!({ "q": 1 })),
        "golden-run",
    );
    assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);

    let bytes = to_canonical_bytes(&serde_json::to_value(&run.journal).unwrap());
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(
        text,
        r#"{"entries":[{"event":{"kind":"model-call","request":{"model":"mock","prompt":"answer: {\"q\":1}","temperature":0.0},"response":{"identity":{"advertised_model":"mock","endpoint":"in-process","provider":"mock","request_schema_version":"1","revision":"mock-r1","sampling":{"temperature":0.0}},"text":"mock:7f73d3e79aee74b2"}},"node_path":"root","seq":0}],"run_id":"golden-run"}"#
    );
}
