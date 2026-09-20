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
| `entelechy-eval` | Tasks, checkers, statistics, split discipline, holdout gate (§9) | **Implemented** |
| `entelechy-policy` | Authority checks, Gate evaluation, policy adapter (§8.3, §5.4, Q6) | **Implemented** |
| `entelechy-objective` | Objective interview & GoalSpec compiler (§5.5) | **Implemented** |
| `entelechy-capability` | Capability discovery & CapabilityGraph (§13.1) | **Implemented** |
| `entelechy-failure` | Failure ontology, clustering, causal evidence (§10) | **Implemented** |
| `entelechy-design` | Initial synthesis, DesignHypothesis, patches, repair engine (§11) | **Implemented** |
| `entelechy-search` | Explorer strategies, archives, budgets, stopping rules (§11.7, §11.8) | **Implemented (study driver)** |
| `entelechy-assurance` | Static assurance, holdout orchestration, SafetyCase (§14.1) | **Implemented** |
| `entelechy-release` | Release bundles, maturity state, promotion records (§14) | **Implemented** |
| `entelechy-ops` | Online eval, drift, incidents, re-study triggers (§15) | **Implemented** |
| `entelechy-server` | API/server mode (§16.2) | **Implemented (core)** |
| `entelechy-identity` | Principals, authn context, approval signatures, SoD (§5.8, §17.2) | **Implemented** |
| `entelechy-effects` | Effect taxonomy, operation keys, transactional state machine, reconciliation (§7.6, §8.5) | **Implemented** |
| `entelechy-bench` | Benchmark provenance, split lineage, contamination, manifests (§9.5, §21.1) | **Implemented** |
| `entelechy-protocol` | Versioned API/plugin manifests, negotiation, compatibility fixtures (§16.6) | **Implemented** |

## Implemented today (Phase 0 / early Phase 1 core)

The vertical slice the PRD requires for Phase 0 — *execute → journal → replay* on
a single-agent IR — runs end to end:

```
entelechy objective     # clarification interview -> GoalSpec -> EvalContract -> sign-off (§5.5)
entelechy capability    # CapabilityGraph, CD-7 attestation, gaps (§13.1)
entelechy eval          # EvalContract, power check, holdout gate + firewall (§9)
entelechy design --validate <design.json>   # static compiler invariants (§7.4)
entelechy study         # full improve loop on the simulated helpdesk (§21)
entelechy trace         # render a run's journal as a trace (§16.2)
entelechy demo          # execute -> journal -> replay + effect-safety conformance
entelechy replay --design <d.json> --journal <j.json>   # deterministic replay (§8.2)
entelechy diff          # structural IR diff, pinned-node aware (§16.2)
entelechy assure        # assurance compiler + SafetyCase (§14.1)
entelechy release       # build a bundle, promote L0->L2 through the gate (§14)
entelechy requirements  # the §17.6 cross-cutting registry
# planned: serve (HTTP transport over the entelechy-server dispatcher, §16.2)
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
- **Policy-enforced execution (§8.3, §7.5):** when a runtime is given a
  `PolicyConfig`, every consequential tool effect is authorized at its boundary
  before dispatch (forbidden/ungranted → typed `PolicyDenied`; IR-I3
  tainted-without-gate denied; unmet approval/irreversibility obligations fail
  closed). Model calls are treated as egress (IR-I9): a value carrying a
  confidentiality label may only enter a prompt if the selected provider is
  approved for that label (Q19), else the call is denied before dispatch.
  Decisions are recorded in a `DecisionLog` kept off the replay journal, so replay
  stays byte-identical.
- **Evaluation (§9):** task model with provenance/difficulty/split (EV-1),
  programmatic checkers (EV-4), a governed `EvalContract` with constraint classes
  (§5.6) and per-split power warnings (EV-15); statistics — paired bootstrap
  comparison (EV-9), the rule-of-three / Wilson negative-goal upper bound (§5.6),
  risk-class epsilon defaults (Q11), and the §9.4 minimum-detectable-effect
  table; the audited, budgeted `HoldoutVault` gate with the identity leakage
  firewall (EV-14/EL-1/§9.6) — design/search identities are refused, responses
  are coarse pass/fail with an interval, and every query is audited; plus LLM-free
  retrieval metrics — recall@k, precision@k, MRR and nDCG@k (MK-4) — for tuning
  retrieval before spending model budget.
- **Traceability (§17.5/§17.6):** the 27 cross-cutting requirement IDs as data,
  with verification owner and first-enforced phase.

- **Benchmark governance (§9.5, §21.1):** `BenchmarkManifest` with per-task
  provenance and split lineage and a sealed set of holdout content hashes
  (EL-2); a cross-split contamination scan; the Q18 detector (normalized-text
  hashing, word-trigram Jaccard, embedding cosine with the Q18 thresholds); and
  the signed `StudyPlan` pre-registration whose content hash is its commitment,
  so any post-hoc change is detectably material (§21.1).
- **Study driver (§21.1, EV-13):** candidate accounting that counts every
  evaluated candidate against the budget (no hidden trials), the
  rolling-validation acceptance rule (accept only if a change wins on tune *and*
  is confirmed on validation, else revert as inconclusive), and the Phase 0
  stopping rule (budget cap + a single non-binding futility check at half, no
  early stopping for success).

- **Design synthesis & repair (§11):** mandatory single-agent baseline synthesis
  (11.1); typed edit operators (11.5) — change model, mutate prompt, set
  temperature, add verify, delete node — applied as immutable patches that
  respect pinned nodes (IR-I7); and the DesignHypothesis record (11.2) with the
  Q3 rule that a low-confidence (diagnostic) hypothesis can never confidently
  reject, only end inconclusive.

- **Policy engine (§8.3, Q6, Q19):** a native policy core that authorizes effects
  at the boundary — forbidden/ungranted capability denial (IR-I2), network-scope
  egress checks, provider approval by data classification (Q19/7.5), IR-I3
  tainted→privileged denial without a Gate, and approval/two-person obligations
  for consequential and irreversible effects (5.4) — behind a `PolicyEngine`
  adapter trait, with recorded decisions and a versioned `PolicySnapshot`.
- **Identity & approvals (§5.8, §17.2, §14.5):** principals with the meta-layer
  "no runtime authority" rule (11.9); an authentication context whose credential
  expiry and uncertain-clock both fail closed (AU-1/AU-3); hash-bound approval
  signatures that revalidate at promotion and go `Stale` on any material change
  (14.5/Q17), `Expired`, `Revoked` or `BadSignature`; a key ring with rotation
  and revocation where revoked keys can't sign (AU-2); and separation of duties.
- **Failure intelligence (§10):** the versioned failure ontology (10.1); an
  analyzer that clusters failed observations by structural signature, assigns a
  calibrated class distribution with entropy and an unresolved-cause bucket,
  distinguishes system from evaluation failure, and keeps low-confidence
  diagnoses diagnostic-only (10.2, Q3); the class→complexity-level eligibility
  map (11.4); and Q3 budget allocation (proportional to probability × impact with
  a reserved share for high-entropy clusters, skipping evaluation-defect clusters).
- **Effect safety (§7.6, §8.5, RS-1):** operation keys with logical attempts,
  the planned→authorized→dispatched→acknowledged→reconciled state machine over a
  durable write-ahead log, read-back reconciliation for idempotent/read-back
  effects and reconciliation-required for the rest (Q16), and a crash-injection
  conformance suite proving no silent duplicate consequential effect at any
  transition. Surfaced as step 5 of `entelechy demo`.

- **Assurance & release (§14):** an assurance compiler that runs static IR
  validation and assembles a `SafetyCase` whose claims are supported per class
  (structural→proof, behavioral→upper-bound test within epsilon, operational→cap,
  PRD 5.6); the L0–L5 maturity ladder with monotonic promotion, holdout/negative-
  goal blocking, recorded overrides (14.3), approval revalidation at promotion
  that fails on any stale binding (14.5), and RB-2 rollback-readiness for L4/L5;
  and a deployment controller with rollback/revocation whose stop authority is
  separate from promote authority and whose auto-rollback triggers are declared
  (14.6). The lifecycle state machine encodes Appendix A including reverse
  transitions.

- **Capability discovery (§13.1, CD-7, Q13):** a `CapabilityGraph` where declared
  effects are untrusted hints — unattested capabilities take the conservative
  external+irreversible default and are denied write authority; operator
  attestations expire after 90 days or on any version change; plus gap analysis
  (CD-4) and drift events (CD-6).
- **Protocol compatibility (§16.6, PC-1/2/3, Q29):** highest-common version
  negotiation that fails closed on no overlap (PC-1), a secure-negotiation path
  that refuses to silently drop a required security guarantee unless the operator
  opts in and records it (PC-2), plugin-manifest load checks that fail closed
  (PC-3), and the Q29 two-minor-plus-six-month deprecation window.

- **Objective compiler (§5.5):** a `GoalSpec` with per-field provenance
  (stated/inferred/defaulted, OC-1), classified negative goals (OC-2), the bounded
  clarification interview that ranks by expected impact and defers the rest as
  assumptions (OC-3), the assumption ledger requiring a falsifying task per
  inferred assumption (OC-4), the constrained-optimization / lexicographic /
  human-choice selection policy (5.7), and immutable content-addressed sign-off
  that refuses an unfalsifiable ledger and detects new lineage branches (OC-8).
- **Operations (§15):** drift detection with a z-shift statistic and evidence
  (OP-3), incidents converted to permanent regression tasks (OP-4/EV-12), bounded
  re-studies that never auto-promote (OP-5), and per-scope quotas with hard stops
  enforced outside model output (OP-7).
- **Server core (§16.2):** a transport-independent API dispatcher that fails
  closed on unsupported protocol (PC-1), authenticates with fail-closed expiry
  (AU-1), and enforces per-operation authorization separately from authentication
  (17.2) — the core a future HTTP/gRPC layer wraps.

All 21 workspace crates are now implemented at Phase-0 / early-Phase-1 depth
(134 tests). Later phases deepen them (e.g. real HTTP transport, Cedar policy
adapter, task synthesis, cluster mode) per PRD §21.2.

Try it:
- `entelechy eval` — the helpdesk contract, the power check and the holdout gate + firewall.
- `entelechy study` — a full Phase 0 loop: baseline → signed StudyPlan → structural
  safety → one repair hypothesis with candidate accounting and rolling-validation
  confirmation → holdout gate, on the simulated helpdesk.

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
