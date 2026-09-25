//! Whole-pipeline integration tests (PRD 3.1, 8.2, 5.9).

use std::collections::HashMap;

use entelechy_artifacts::ArtifactId;
use entelechy_design::synthesize_single_agent;
use entelechy_gateway::{MockModel, NativeToolGateway};
use entelechy_ir::{AuthorityEnvelope, EffectMetadata, Value};
use entelechy_runtime::{Engine, RunStatus};

/// design → IR validation → runtime execute → deterministic replay.
///
/// This crosses five crates in one flow and asserts the core PRD 8.2 contract:
/// replaying a recorded journal reproduces the run with no divergence.
#[test]
fn synthesize_execute_and_replay_is_deterministic() {
    // 1. Synthesize a single-agent baseline (design).
    let mut authority = AuthorityEnvelope::empty();
    authority.capabilities.insert("draft_reply".into());
    let program = synthesize_single_agent(authority, "mock-small", "resolve: {input}");

    // 2. It is structurally valid against an (empty) capability catalog (IR).
    let catalog: HashMap<String, EffectMetadata> = HashMap::new();
    let violations = entelechy_ir::validate(&program, &catalog);
    assert!(
        violations.is_empty(),
        "baseline should be structurally valid, got {violations:?}"
    );

    // 3. Execute it on the interpreter with the deterministic mock model (runtime).
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    let mut engine = Engine::new(&model, &mut tools);
    let input = Value::trusted(serde_json::json!({ "ticket": "help" }));
    let run = engine.execute(&program, input.clone(), "integration-run");
    assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);
    // The single model call was journaled (RK-2).
    assert_eq!(run.journal.entries.len(), 1);

    // 4. Replay against the recorded journal: no divergence (PRD 8.2).
    let replayed = engine
        .replay(&program, input, &run.journal)
        .expect("replay must not diverge from its own journal");
    assert!(replayed.is_some());
}

/// Content addressing is a stable, cross-crate identity contract (PRD 5.9): the
/// same design hashes to the same id; a material change hashes to a different one.
#[test]
fn identical_designs_share_a_content_address() {
    let auth = || {
        let mut a = AuthorityEnvelope::empty();
        a.capabilities.insert("draft_reply".into());
        a
    };
    let a = synthesize_single_agent(auth(), "mock-small", "resolve: {input}");
    let b = synthesize_single_agent(auth(), "mock-small", "resolve: {input}");
    let c = synthesize_single_agent(auth(), "mock-small", "different template: {input}");

    let id_a = ArtifactId::of(&a).unwrap();
    let id_b = ArtifactId::of(&b).unwrap();
    let id_c = ArtifactId::of(&c).unwrap();

    assert_eq!(id_a, id_b, "identical designs must share an id");
    assert_ne!(id_a, id_c, "a changed prompt must change the id");
    // The id round-trips through its wire form.
    assert_eq!(ArtifactId::parse(&id_a.to_string()).unwrap(), id_a);
}
