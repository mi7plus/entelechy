//! Benchmarks for the content-addressing hot path (PRD 5.9, Q22).
//!
//! `to_canonical_bytes` and `ArtifactId::of` run on every artifact write and on
//! every id comparison in search, so they are worth a regression baseline. Run
//! with `cargo bench -p entelechy-artifacts`.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use entelechy_artifacts::{to_canonical_bytes, ArtifactId};
use serde_json::json;

/// A representative nested artifact payload (a small design-like document).
fn sample() -> serde_json::Value {
    json!({
        "header": { "schema_id": "entelechy.design", "schema_version": "1.2.3", "tenant": "local" },
        "nodes": (0..32).map(|i| json!({
            "id": format!("node-{i}"),
            "kind": if i % 2 == 0 { "llm" } else { "code" },
            "tags": ["a", "b", "c"],
            "meta": { "z": i, "a": i * 2, "m": format!("value-{i}") }
        })).collect::<Vec<_>>(),
        "edges": (0..31).map(|i| json!({ "from": i, "to": i + 1 })).collect::<Vec<_>>(),
    })
}

fn bench(c: &mut Criterion) {
    let value = sample();
    c.bench_function("to_canonical_bytes/32-node-doc", |b| {
        b.iter(|| to_canonical_bytes(black_box(&value)))
    });
    c.bench_function("ArtifactId::of/32-node-doc", |b| {
        b.iter(|| ArtifactId::of(black_box(&value)).unwrap())
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
