# Entelechy — architecture & crate map

Entelechy is a compiler and lifecycle platform for synthesizing bounded agentic
systems from objectives (PRD v11). This document maps the Rust workspace to the
PRD and records what is implemented versus scaffolded.

> The authoritative specification is `Entelechy_PRD_v11_FINAL.docx`. Where this
> repo uses requirement IDs (e.g. `IR-I3`, `AC-1`), they refer to that PRD.

## Workspace layout (PRD §24)

| Crate | Responsibility (PRD) | Status |
|-------|----------------------|--------|
| `entelechy-artifacts` | Artifact IDs, lineage, content addressing, canonical serialization (§5.2, §5.9, §17, Q22) | **Implemented** |
| `entelechy-ir` | IR node model, value envelope, effects, authority, static invariants (§7, §5.4, §5.6, §7.5, §7.6) | **Implemented** |
| `entelechy-gateway` | Model/tool gateways, provider identity capture, deterministic mock model (§8.3, §5.10, §7.5) | **Implemented** |
| `entelechy-runtime` | Interpreter, append-only journal, deterministic replay, typed terminal states (§8) | **Implemented** |
| `entelechy-contracts` | Normative requirement registry & traceability (§17.5, §17.6) | **Implemented** |
| `entelechy-cli` | Local CLI workflow (§16.1) | **Implemented (core commands)** |
| `entelechy-policy` | Authority checks, Gate evaluation, policy adapter (§8.3, §5.4, Q6) | Scaffold |
| `entelechy-eval` | Tasks, checkers, environments, statistics, split discipline (§9) | Scaffold |
| `entelechy-objective` | Objective interview & GoalSpec compiler (§5.5) | Scaffold |
| `entelechy-capability` | Capability discovery & CapabilityGraph (§13.1) | Scaffold |
| `entelechy-failure` | Failure ontology, clustering, causal evidence (§10) | Scaffold |
| `entelechy-design` | Initial synthesis, DesignHypothesis, patches, repair engine (§11) | Scaffold |
| `entelechy-search` | Explorer strategies, archives, budgets, stopping rules (§11.7, §11.8) | Scaffold |
| `entelechy-assurance` | Static assurance, holdout orchestration, SafetyCase (§14.1) | Scaffold |
| `entelechy-release` | Release bundles, maturity state, promotion records (§14) | Scaffold |
| `entelechy-ops` | Online eval, drift, incidents, re-study triggers (§15) | Scaffold |
| `entelechy-server` | API/server mode (§16.2) | Scaffold |
| `entelechy-identity` | Principals, authn context, approval signatures, SoD (§5.8, §17.2) | Scaffold |
| `entelechy-effects` | Effect taxonomy, operation keys, transactional state machine, reconciliation (§7.6, §8.5) | Scaffold |
| `entelechy-bench` | Benchmark provenance, split lineage, contamination, manifests (§9.5) | Scaffold |
| `entelechy-protocol` | Versioned API/plugin manifests, negotiation, compatibility fixtures (§16.6) | Scaffold |

## Implemented today (Phase 0 / early Phase 1 core)

The vertical slice the PRD requires for Phase 0 — *execute → journal → replay* on
a single-agent IR — runs end to end:

```
entelechy demo          # validate IR, execute (journaling every effect), replay
entelechy design --validate <design.json>   # static compiler invariants (§7.4)
entelechy replay --design <d.json> --journal <j.json>   # deterministic replay (§8.2)
entelechy requirements  # the §17.6 cross-cutting registry
```

### What is real

- **Content addressing (§5.9, Q22):** RFC 8785 canonical JSON + SHA-256 with a
  digest-algorithm identifier for agility. Object key order and whitespace do not
  affect identity. Blobs hash as raw bytes. `AC-1` schema headers on artifacts;
  authorization-before-blob-resolution in the store (§17.2).
- **IR (§7):** node model (Llm/Tool/Code/Seq/Par/Map/Branch/Loop/Delegate/Verify/
  Mem/Gate/Human), value envelope with taint (§7.5) and confidentiality labels,
  effect taxonomy (§7.6) with conservative defaults for unknown effects (CD-7),
  AuthorityEnvelope (§5.4) with subset/narrowing checks.
- **Static invariants (§7.4):** `validate()` checks IR-I2 (authority), IR-I3
  (tainted → privileged effect needs a Gate), IR-I4 (bounded loops), IR-I6
  (delegation narrows), IR-I10 (effect metadata resolves).
- **Runtime (§8):** tree interpreter, append-only journal (RK-2) recording model
  calls, tool effects (with operation key, request hash, commit status) and
  policy decisions; deterministic replay (§8.2) that reports a divergence point
  (RK-8); typed terminal states — Succeeded / Failed(typed) / BudgetExhausted /
  ReconciliationRequired (§8.6); budget exhaustion as a typed terminal (not a
  model failure).
- **Gateways (§8.3):** `ModelGateway`/`ToolGateway` traits; a deterministic
  `MockModel` for replay & the quickstart (Q7); provider identity captured on
  every call (PV-1).
- **Traceability (§17.5/§17.6):** the 27 cross-cutting requirement IDs as data,
  with verification owner and first-enforced phase.

### Known limitations (tracked)

- Canonical JSON numbers rely on `serde_json` formatting rather than full
  ECMAScript `Number::toString` (RFC 8785 §3.2.2.3). Prefer integers/strings in
  artifacts until the AC-2 golden fixtures land.
- `Par` executes sequentially (deterministic) in this slice; true bounded
  parallelism with recorded scheduling order (RK-7) is Phase 1.
- The full effect state machine, reconciliation and compensation (§8.5) are
  Phase 1/4; Phase 0 records write-ahead intent + operation keys + commit status
  only (PRD §21.2 "effect transactions").

## Design decisions locked from the PRD

- Canonical serialization: RFC 8785 JSON, SHA-256 with algorithm id (Q22).
- `NodeKind`/`Condition` are **externally tagged** in JSON: internal tagging of a
  recursive enum makes serde's serializer type-resolution non-terminating. The
  external form stays human-readable and diffable (IR-I8).
- Every artifact and store object carries a tenant/security-domain id even in
  local mode (default `local`) so later isolation stays possible (§17.2).

## Phase roadmap (PRD §21)

Phase 0 (prove the thesis) is the current target: single agent, interpreter +
journal + replay, CLI only, a simulated support-triage benchmark, a
pre-registered StudyPlan and a holdout gate API. See `docs/phase0.md`.
