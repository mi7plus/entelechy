//! Benchmarks for the interpreter hot path (PRD 8.1).
//!
//! The same interpreter drives search, replay and production (PRD 3.1), so its
//! per-run cost is multiplied across every candidate a study evaluates. This
//! measures a small mixed program (LLM + code + verify) end to end, plus replay
//! against a recorded journal. Run with `cargo bench -p entelechy-runtime`.
// The criterion_group! macro emits an undocumented public symbol; a bench harness
// is not public API, so waive the workspace missing_docs lint here.
#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use entelechy_gateway::{MockModel, NativeToolGateway};
use entelechy_ir::{
    AuthorityEnvelope, CodeNode, LlmNode, Node, NodeKind, Program, Value, VerifyNode,
};
use entelechy_runtime::Engine;

fn program() -> Program {
    Program::new(
        AuthorityEnvelope::empty(),
        Node::new(
            "root",
            NodeKind::Seq(vec![
                Node::new(
                    "classify",
                    NodeKind::Llm(LlmNode {
                        model: "mock".into(),
                        prompt_template: "classify: {input}".into(),
                        temperature: 0.0,
                    }),
                ),
                Node::new(
                    "tag",
                    NodeKind::Code(CodeNode {
                        function: "tag_ok".into(),
                    }),
                ),
                Node::new(
                    "check",
                    NodeKind::Verify(VerifyNode {
                        checker: "has_text".into(),
                    }),
                ),
            ]),
        ),
    )
}

fn bench(c: &mut Criterion) {
    let prog = program();

    c.bench_function("engine/execute/seq-llm-code-verify", |b| {
        b.iter(|| {
            let model = MockModel::new();
            let mut tools = NativeToolGateway::new();
            let mut e = Engine::new(&model, &mut tools);
            e.register_code("tag_ok", |v| Ok(v.data.clone()));
            e.register_checker("has_text", |_| true);
            let r = e.execute(
                black_box(&prog),
                Value::trusted(serde_json::json!({ "text": "hello" })),
                "bench",
            );
            black_box(r.status);
        })
    });

    // Replay against a recorded journal (PRD 8.2) — the search/verification path.
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    let mut e = Engine::new(&model, &mut tools);
    e.register_code("tag_ok", |v| Ok(v.data.clone()));
    e.register_checker("has_text", |_| true);
    let recorded = e.execute(
        &prog,
        Value::trusted(serde_json::json!({ "text": "hello" })),
        "bench",
    );

    c.bench_function("engine/replay/seq-llm-code-verify", |b| {
        b.iter(|| {
            let model = MockModel::new();
            let mut tools = NativeToolGateway::new();
            let mut e = Engine::new(&model, &mut tools);
            e.register_code("tag_ok", |v| Ok(v.data.clone()));
            e.register_checker("has_text", |_| true);
            let out = e.replay(
                black_box(&prog),
                Value::trusted(serde_json::json!({ "text": "hello" })),
                black_box(&recorded.journal),
            );
            black_box(out.is_ok());
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
