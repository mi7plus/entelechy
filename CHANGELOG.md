# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
Semantic Versioning with explicit compatibility promises for the IR, artifact
schemas and journal format (PRD §5.9, §16.5, §16.6).

## [Unreleased]

Pre-1.0. Interfaces are still provisional until the 1.0 contract freeze (PRD §21,
Phase 1). This section tracks the initial implementation of the PRD v11 baseline.

### Added
- **Rollback disaster-recovery drill (PRD §14.6, RB-1/RB-2).** A new
  `entelechy-release::drill` module mechanizes the operational drill those two
  requirements call for: `rollback_drill()` runs the real promote → deploy →
  trigger → rollback → revoke sequence end to end and returns a `RollbackDrillReport`.
  It proves **RB-2** — a production (L4/L5) promotion with no tested rollback target
  *and* no approved no-rollback rationale is refused at the real promotion gate,
  while an otherwise-identical rollback-ready bundle (target or rationale) passes —
  and **RB-1** — stop authority is separate from promote authority (an on-call
  stopper cannot deploy; a releaser cannot roll back or revoke), the declared
  automatic rollback triggers fire while an undeclared signal does not, a rollback
  recovers the prior known-good release as active, and emergency revocation of the
  active release is a fail-safe stop (the system then serves nothing). The `entelechy
  drill` command now takes `--kind restore|rollback|all` (default `all`): `restore`
  runs the DR-1 backup/restore drill, `rollback` runs this one, and `--out` persists
  each drill's report (`rollback-report.json`). The RB-1/RB-2 operational-drill
  verification is recorded in `phase_gate_evidence()` now (they are first enforced at
  P4), so the P4 gate is already covered when the gate advances; the current P1 gate
  is unchanged. Two new drill tests (the composite RB-1/RB-2 invariants, determinism).
- **Backup → restore → verify disaster-recovery drill (PRD §17.3, DR-1).** A new
  `entelechy-artifacts::backup` module mechanizes DR-1 — "backup and restore
  preserve hashes, lineage and authorization metadata." A `BackupPacket` captures
  the durable state (every content-addressed object with its owning tenant, the
  append-only metadata/event log, the lineage edge graph and the erasure ledger)
  and serializes to cold-storage bytes; `run_restore_drill` restores it *through*
  those bytes and verifies each DR-1 invariant: every object is re-hashed to the
  address it was filed under (a tampered backup fails closed with
  `RestoreError::IntegrityFailure` rather than loading), each restored object keeps
  its owning tenant so the owner can read it and every other tenant is refused
  (authorization re-enforced — PRD §17.2), and the lineage graph and event log
  round-trip unchanged. The restored erasure ledger is replayed so crypto-shredded
  data can never reappear (DR-2). Privileged `InMemoryStore::export` and
  `InMemoryMetadataStore::export`/`import` back the whole-store read a backup needs,
  deliberately distinct from `BlobStore::get` (possession of a hash is never
  authority). A new `entelechy drill [--out <dir>]` operator command builds a
  realistic slice of durable state across two security domains (a signed objective,
  a design, a semantics-preserving migration and a promoted release under `local`;
  an isolated design plus a crypto-shredded subject under `tenant-b`), runs the
  drill, prints the per-invariant `RestoreReport`, proves the fail-closed property
  by rejecting a tampered backup, and with `--out` persists the backup packet and
  the restore report as an auditable verification record. Five new tests cover the
  happy path, corruption rejection, the re-enforced tenant firewall, erasure
  survival and determinism. This **retires the DR-1 TR-1 waiver**: the phase gate
  now carries a DR-1 verification record instead (two waivers remain — PV-2, DP-1).
- **Closed operate → re-study → release loop (PRD §14.3, §21, OP-5).** The online
  loop's opened `ReStudy` is no longer inert: a new `entelechy-cli::restudy` module
  turns the drift re-study into a real re-optimization and drives it to a governed
  release decision. It builds a repaired candidate as a minimal typed patch —
  the deployed design with its over-restrictive refund-refusing `Gate` removed, so
  a refund ticket gets a safe drafted escalation instead of being left unresolved —
  while the authority envelope is unchanged (`refund` stays forbidden and ungranted),
  so the structural negative goal is fully preserved: the candidate still has no way
  to issue a refund. The candidate is exercised on the incident-derived **regression
  suite** (`Provenance::Incident`, EV-12, scored directly — these are regression
  coverage, never independent holdout evidence, PRD §9.5), fixing the class 0/8 →
  8/8, and then gated on a sealed, incident-free **independent holdout** through the
  assurance-only release gate (`HoldoutVault`, EV-14, §9.6), which passes on the
  measured ~50pp improvement. Critically, even a passing candidate is **never
  auto-promoted** (`ReStudy::auto_promotes()` is always `false`): it stops at the
  release gate awaiting human approval (OP-5, the PRD non-goal of autonomous
  production promotion). `entelechy ops` now runs the drift re-study and prints the
  closed-loop outcome; `--out <dir>` persists it in `ops-report.json` (the regression
  fix, the coarse gate verdict, and `promoted: false`) and records the release-gate
  decision as a final entry in the hash-chained `audit-log.json`, so
  `entelechy audit <dir>` re-verifies the whole operate → re-study → release trail.
  `ops.rs` grew a reusable `execute_on`/`resolves` scoring pair so the loop and the
  re-study's holdout gate score runs through the same real runtime and `EvalContract`
  checkers.
- **Online-eval / drift / incident loop (PRD §15, §21.4).** A running
  `OnlineEvalLoop` (`entelechy-ops::online`) composes the crate's stateless
  primitives into the P4 "measure and operate in production" story. It consumes a
  stream of production `RunObservation`s and user `Feedback` and emits a typed,
  auditable `OpsEvent` log: each run is sampled for online evaluation at a bounded
  rate (OP-2); per drift signal it holds a frozen baseline and a rolling recent
  window and re-runs `detect_drift` as sampled values arrive (OP-3); on detected
  drift — or a user-satisfaction breach past a floor — it captures an `Incident`
  from the triggering trace, turns it into permanent regression coverage (OP-4,
  EV-12), and opens a *bounded* `ReStudy` whose candidates wait at the release gate
  and never auto-promote (OP-5, §14.3). Every re-study is charged against a
  `QuotaLedger`; once the budget is spent the loop emits a hard-stop event instead
  of opening another (OP-7, §17.4). A drift signal latches, so a sustained regime
  opens one re-study rather than one per run, and the loop is deterministic — the
  same observations in the same order always yield the same event stream, so an
  operator can replay and audit exactly why each incident and re-study opened. A
  new `entelechy ops [--out <dir>]` CLI command drives the loop with **measured**
  scores, not hand-supplied numbers (PRD §9): it executes a deployed single-agent
  triage design over a stream of production tickets through the real runtime and
  scores each run's output against a live `EvalContract` with programmatic checkers
  (EV-4), feeding the resulting `task_success` into the loop as the drift signal
  and deriving user feedback from each run's real resolution. The agent resolves
  reply tickets but refuses a refund at the gate (a structural negative goal), so a
  refund surge drops the measured `task_success`; the loop detects that drift from
  the scores, opens an incident, captures regression coverage (EV-12) and queues a
  bounded re-study. The command prints the per-run scores and the decision trail;
  `--out <dir>` persists the governed output (PRD 17.2) — the full event log, a
  structured `ops-report.json` of the incidents (with their trace refs), the
  regression tasks and the re-studies, and a tamper-evident hash-chained
  `audit-log.json` of every decision that the report commits to by chain head, so
  `entelechy audit <dir>` re-verifies both the chain and that commitment. The
  scoring and persistence wiring lives in the CLI, keeping `entelechy-ops` a leaf
  crate.
- **Retrieval IR node + level-2 search unlock (PRD 11.4 level 2, §12).** A new
  `Retrieval` node (`entelechy-ir`) fetches the `top_k` most relevant entries of a
  memory tier for a query built from the current value. Statically it is a read —
  no capability, no authority — and its output enters as tainted content carrying
  forward the confidentiality labels of whatever it retrieves (IR-I9); a
  downstream privileged effect must still be gated (IR-I3). The runtime executes it
  against `TieredMemory::retrieve`, a deterministic lexical-overlap search with a
  stable (score-desc, key-asc) order; being an in-process read it records no
  journal event and replays/counterfactuals reproduce it exactly (RK-2). A new
  `Engine::seed_memory` stages the semantic knowledge base it reads. The design
  layer gains the typed `AddRetrieval` operator (prepend a retrieval to the root
  `Seq` — retrieve, then reason), and the search proposer now emits it at level 2,
  closing the gap where a `knowledge.retrieval-miss` was eligible for retrieval but
  had no operator. The compiled backend reports `Retrieval` as unsupported (the
  interpreter is its reference executor for the Q10 conformance gate).
- **TR-1 traceability check in CI (PRD §17.5).** `entelechy requirements --check`
  builds the authoritative phase-gate evidence (`entelechy_contracts::phase_gate_evidence`)
  and fails non-zero if any requirement first enforced at or before the current
  gate phase (P1) has no verification record and no active waiver. A new
  `traceability` CI job runs it on every push/PR. Eleven P0/P1 requirements are
  linked to their verifying tests (DR-1 since the backup/restore drill above); the
  two study/approval requirements not yet mechanized (PV-2, DP-1) carry scoped,
  expiring waivers with compensating controls. Adds a runtime test that a Human
  node fails closed and never implies
  approval (RS-2), and contracts tests asserting full phase coverage and that no
  StaticProof requirement is waived (TR-1).
- **1.0 contract freeze (Phase-1 exit, PRD §21).** A normative freeze of the
  externally observable contracts: `docs/contracts-1.0.md` documents each frozen
  schema (canonical serialization + artifact header, Design IR incl. task/checker
  nodes and `AuthorityEnvelope`, journal wire format, native model-request shape,
  failure ontology, protocol/plugin compatibility), its version constant and its
  compatibility promise. New version constants where they were missing
  (`JOURNAL_FORMAT_VERSION`, `NATIVE_REQUEST_SCHEMA_VERSION`,
  `CURRENT_PROTOCOL_VERSION`); the journal golden now pins its version next to its
  bytes. A machine-readable `entelechy_contracts::FROZEN_CONTRACTS` registry mirrors
  the document, and a cross-crate gate (`entelechy-cli/tests/contract_freeze.rs`)
  fails CI if a frozen version and its owning constant drift apart — so a frozen
  format can only change as a deliberate, reviewed version bump (AC-1, AC-2, PC-1,
  PC-2, PC-3, RK-2, RS-1, PV-1).
- **Multi-agent designs (Phase 1, PRD 11.4 level 4/5).** A multi-agent synthesizer
  (`synthesize_multi_agent`, a coordinator over parallel scoped `Delegate`
  sub-agents) and the level-4/5 typed edit operators `SplitParallel` (fan a single
  agent into a parallel committee) and `AddDelegate` (wrap a subgraph in a
  narrowed-authority sub-agent). Delegation narrowing is enforced at apply time
  against the authority in force at the wrap site, including nested delegates
  (A2 / IR-I6); widening is rejected. A `DesignHypothesis::decomposition`
  constructor ties these operators to the `reasoning.decomposition` failure class
  the Architecture Explorer unlocks them on. Cross-crate tests execute and replay
  the synthesized multi-agent programs end to end.
- **Study-loop operator proposal** (`entelechy_search::propose`): the search driver
  now turns a diagnosed failure class into the next design mutation, escalating
  single-agent → verification → parallel committee → multi-agent delegation only
  when the class makes the level eligible and the Architecture Explorer allows the
  unlock (within ceiling/budget, without adding authority — Q4). Level-5 delegation
  proposals build a validly-narrowed reasoning-only sub-agent authority (A2/IR-I6).
  The `study` CLI command now reports the proposed level and operators.
- **Iterative, escalating study loop.** The `study` command now runs a real
  multi-candidate search: each round it re-diagnoses the best design's remaining
  failures, proposes the next mutation, evaluates it under rolling validation, and
  adopts or escalates to the next complexity level — converging when no failure
  remains, the budget is spent, or an unlock needs approval. The demo suite now
  carries two failure classes (reply → verification/level 3, multi-part →
  decomposition/level 4), so a run visibly escalates single-agent → verification →
  parallel multi-agent committee before the holdout gate. Adds a
  `Symptom::IncompleteDecomposition` mapping to `reasoning.decomposition`.
- **Committee reduce step.** `SplitParallel` is now a full map/reduce: it produces
  `Seq[Par[specialists], coordinator]`, where a coordinator `Llm` aggregates the
  agents' outputs into one synthesized value instead of returning a raw array
  (PRD 11.4 level 4). Executes end to end with any model.
- **`entelechy study --out <dir>`** persists the run's artifacts: the final best
  design IR (`design.json`, re-validatable and diffable) and a `study-report.json`
  recording the StudyPlan commitment, baseline/best content addresses, final
  complexity level, budget usage, every hypothesis with its result, a typed
  `derived-from` lineage chain with one edge per accepted step (baseline → v1 →
  … → best, PRD 5.2), and the holdout gate outcome — a reproducible, auditable
  record (PRD principle 6, 21.1). It also writes a tamper-evident, hash-chained
  `audit-log.json` (PRD 17.2) recording the plan commitment, each hypothesis
  result, and the holdout outcome; the report commits to the audit chain head.
- **Server `validate_design` operation.** The local API (`entelechy serve`) now
  exposes Design IR validation as an operation (PRD 16.2: UI actions map to API
  operations): POST a design as the payload, get back `{ valid, violations }`.
- **`entelechy counterfactual`** exposes counterfactual replay (PRD 10.2, RK-8)
  from the CLI: replay a recorded journal up to a node, substitute its output, and
  re-execute everything downstream live — reporting the divergence point, the
  number of live rollouts, and the recomputed output.
- **`entelechy audit <dir>`** re-verifies a persisted audit log's hash chain and
  that a study report's `audit_head` still matches the chain head, exiting
  non-zero on any tampering or mismatch (PRD 17.2). It prints the current chain
  head to record as an external anchor, and `--anchor <hash>` verifies a
  previously-held anchor is still in the chain — catching a full rewrite that an
  internal chain check alone cannot.
- **`entelechy committee` CLI command.** Builds a multi-agent committee, runs it
  against a model, and prints each sub-agent's output plus the coordinator's
  synthesized answer. Uses a live OpenAI-compatible model when built with
  `--features openai` and `ENTELECHY_BASE_URL`/`ENTELECHY_MODEL` are set; otherwise
  the deterministic mock model, so it always runs. `--agents N` sizes the
  committee, `--analyst`/`--responder`/`--coordinator` override prompts, and
  `--out <dir>` persists the design IR and recorded journal so the run can be
  replayed (`entelechy replay`) and reproduces deterministically (PRD 8.2/RK-2).

- Workspace implementing the PRD v11 architecture (§24), plus a dedicated
  `entelechy-integration` crate for cross-crate pipeline tests.
- Property tests (`proptest`) for the content-addressing trust root, criterion
  benchmarks for the canonicalization and interpreter hot paths, runnable
  doctests, and `cargo fuzz` targets for canonical JSON and IR deserialization.
- CI: an MSRV (1.85) build job and a non-blocking coverage job; the RUSTSEC
  advisory scan now runs on push/schedule only (not on PRs).
- A pull-request template surfacing the CI gates and DCO sign-off.
- `missing_docs = "warn"` as a workspace lint, so the (already universal) public-API
  documentation discipline is enforced and the rustdoc `-D warnings` gate fails on a
  regression.
- Golden format-stability tests that pin the canonical-JSON bytes, the SHA-256
  `ArtifactId` digest, and the journal wire format for fixed fixtures — turning the
  §5.9 / §16.5–16.6 compatibility promises into enforced tests (an accidental
  format change fails CI; a deliberate one requires bumping the serialization
  version and updating the golden value).

### Fixed
- HTTP server hardening (loopback DoS): the request body is now capped
  (`413` above 1 MiB) instead of allocating `Content-Length` verbatim, header
  count is bounded (`431`), and per-connection read/write timeouts prevent a
  single stalled client from wedging the serial accept loop.
- OpenAI gateway: the client now bounds the TCP dial with `connect_timeout`, so a
  wrong `--base-url` fails within the timeout instead of blocking on the OS default.
- Bootstrap PRNG: guard the one seed that would zero the xorshift state (which would
  collapse the confidence interval); such a seed now falls back to a non-zero state.
- IR with node model, value envelope (taint + confidentiality), effect taxonomy,
  AuthorityEnvelope and static compiler invariants (§7, §5.4, §5.6).
- Content-addressed artifacts (RFC 8785 + SHA-256, digest agility), schema
  versioning, tenant-scoped stores, deployment profiles, erasure ledger and an
  append-only metadata store (§5.9, §17).
- Runtime: interpreter, append-only journal, deterministic replay, counterfactual
  replay, typed terminal states, memory tiers, cluster work queue + worker pool,
  and a compiled backend with an interpreter-vs-codegen conformance/Q10 gate
  (§8, §10.2, §12, §17, Q10).
- Evaluation subsystem (contract, power analysis, holdout gate + firewall, stats,
  retrieval metrics, adversarial generation, judge calibration) (§9, MK-4).
- Policy engine (native core + Cedar-style adapter), identity/approvals with a
  tamper-evident audit log, gateways (model/tool + WASM sandbox), capability
  discovery + tool-synthesis governance, failure analyzer, design/search,
  assurance + release ladder, objective compiler, benchmark governance + task
  synthesis, protocol negotiation, requirement traceability, and a local HTTP API.
- OpenAI-compatible model gateway for self-hosted OSS models (Ollama/vLLM/
  llama.cpp/LM Studio), with an optional TLS transport for hosted providers (Q7).
- CI (build/test on Linux + Windows, feature matrix, fmt, clippy, `cargo deny`
  licence audit) with SHA-pinned actions and Dependabot; project-health docs.

[Unreleased]: https://github.com/mi7plus/entelechy/commits/main
