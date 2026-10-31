# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
Semantic Versioning with explicit compatibility promises for the IR, artifact
schemas and journal format (PRD §5.9, §16.5, §16.6).

## [Unreleased]

Pre-1.0. Interfaces are still provisional until the 1.0 contract freeze (PRD §21,
Phase 1). This section tracks the initial implementation of the PRD v11 baseline.

### Added
- **Level-2 retrieval repair discovered end to end — the searched loop now unlocks
  retrieval, and the levels are a prior, not a ladder (PRD §12, §11.4, §11.3, §21.1).**
  The level-2 vocabulary already existed (the `AddRetrieval` operator, the proposer's
  level-2 emission, and the `knowledge.retrieval_miss` class eligible at `[2]`), but no
  run exercised it: the executor treated a `Retrieval` node as inert, the world had no
  knowledge-dependent ticket, and the repair loop's single monotonic complexity cursor
  made a level-2 fix unreachable once a higher-level fix had been accepted. This closes
  all three gaps, mirroring what `AddTool`/`resolve_all` did for decomposition:
  - the helpdesk world gains a `Knowledge` ticket (`needs_retrieval`) resolved only once
    a reply is drafted **and** the needed durable fact has been retrieved, plus a
    `retrieve(tier, top_k)` effect that surfaces the fact only from the *semantic* tier
    with a positive budget — a misconfigured retrieval finds nothing (`entelechy-domain::world`);
  - the Design-IR executor now honors a `Retrieval` node by applying that effect — a read
    that adds no authority (`entelechy-domain::exec`);
  - the repair loop (`searched_study`) now tracks the complexity level **per failure
    class** instead of with one global cursor, so each class escalates its own ladder.
    A level-2 retrieval miss is therefore discovered *after* a level-3 verify, and a
    stuck class no longer strands the others.
  The unified study gets its own three-class suite (reply/knowledge/decomposition),
  leaving `real_study`'s PV-2 drift narrative on the shared `helpdesk_suite` untouched.
  The loop now discovers **three** repairs by execution — a `Verify` (level 3), a
  `Retrieval` (level 2) and a `resolve_all` `Tool` (level 5) — with `accepted_levels`
  `[3, 2, 5]`, deliberately not level-ordered. `searched_study()` gains
  `baseline_knowledge_unresolved`, `discovered_retrieval` and
  `discovered_resolves_knowledge` invariants; the CLI narrative reports them. Adds three
  unit tests (two world, one executor), updates the searched-study tests, and keeps the
  full suite green (350 tests).
- **Part-resolving edit operator — the searched loop now discovers the
  *decomposition* repair too (PRD §11.5, §11.3, §21.1).** The unified study
  (`study --searched`) previously discovered only the reply-class fix by execution;
  the decomposition class still relied on the hand-authored `resolve_part` committee
  in `real_study`, because the generic proposer's level-4 operator emits a reasoning
  committee of `Llm` sub-agents that resolve nothing when run. This closes that gap.
  A new typed edit operator `entelechy_design::EditOp::AddTool { id, capability, args }`
  appends an *acting* step (a `Tool` invocation) to the root `Seq` — the smallest
  level-1 change for a repair whose fix is a capability call rather than more
  reasoning. The proposer (`entelechy_search`) now realizes the coordination tier
  (level 5) for the `reasoning.decomposition` class as an `AddTool` of the
  `resolve_all` capability the agent already holds (so it widens no authority, IR-I2),
  rather than a bare delegate. The study authority grants `resolve_all` (granted but
  unused by the baseline, which resolves only the first part). The result: the loop
  proposes the level-4 reasoning committee and the executor **rejects** it (the
  honest-divergence contrast is intact — a structural scorer would credit its `Par`),
  then escalates and **discovers** the `resolve_all` tool at level 5, which the
  executor confirms resolves every part. Both Phase-0 repairs are now searched, not
  hand-authored, and `accepted_levels` is `[3, 5]`. `searched_study()` gains
  `discovered_part_resolver` and `discovered_resolves_decomposition` invariants; the
  CLI narrative and `study --searched` output report them. Adds three unit tests
  (two for `AddTool`, one for the proposer), updates the searched-study tests, and
  keeps the full suite green.
- **Unified Phase-0 study — the real repair loop, scored by execution (PRD §11.3,
  §21.1).** Entelechy had two Phase-0 study drivers that each told half the story:
  the simulated `entelechy study` ran the real design-repair proposer loop but scored
  candidates by reading their *structure* (circular — the thing optimized is the thing
  scored), while the real-domain `study --real` scored by *executing* designs against
  the world but used a single hand-authored repair instead of a searched one. A new
  `entelechy-domain::searched` module (`searched_study()`, surfaced as `entelechy study
  --searched [--out]`) unifies them: it drives the **same** proposer loop
  (`entelechy_failure::analyze` → `entelechy_search::propose` →
  `entelechy_design::apply` → `rolling_validation_decision`) but swaps the scoring
  function for the real executor (`exec::resolves`). The demonstration has teeth — the
  loop *discovers* (does not hand-author) the verification repair for the reply class
  and the executor confirms it resolves the tickets, while the level-4 parallel
  committee it proposes for the decomposition class is **rejected** by execution
  (running it resolves nothing — its `Llm` sub-agents carry no part-resolving effect)
  even though the structure-reading scorer would have *credited* it (it "has a `Par`").
  That execution-vs-structure divergence is the point: same search, two scorers, and
  execution is the honest one. The decomposition class therefore stays resolved only by
  the hand-authored `resolve_part` committee in `real_study`; closing that gap needs a
  part-resolving edit operator in the proposer vocabulary (a named next step). Adds
  `entelechy-{failure,design,search}` as `entelechy-domain` dependencies (no cycle; the
  CLI already used all three). Four new tests; `study --searched` prints SATISFIED and,
  with `--out`, persists `searched-study-report.json` and the discovered design.
- **TR-1 phase gate advanced P1 → P4 (PRD §17.5, §21).** `CURRENT_GATE_PHASE` is now
  `Phase::P4`: every cross-cutting requirement first enforced at or before P4 has an
  automated verification record in `phase_gate_evidence()`, and `entelechy
  requirements --check` (the `traceability` CI job) enforces the P4 gate. **No waivers
  remain.** The advance backed the seven P2/P4 requirements that were not yet recorded
  with real code, tests and records (RB-1/RB-2, AU-1/AU-2/AU-3 and SL-1, also P4, were
  recorded in earlier entries):
  - **DX-1 (PRD §16.5) — time to first report.** A new `entelechy-cli::quickstart`
    module makes the new-user path concrete and self-checking: `quickstart_steps()`
    declares the ordered commands a brand-new user runs to reach their first study
    report (install toolchain → build CLI → `init` → `study --real`), each with a human
    time budget summing to well under the 30-minute ceiling, and `run_quickstart()`
    drives the final report-producing step — the Phase-0 real-domain study — in-process,
    measuring wall-clock, and confirms it yields a green report inside budget. A new
    `entelechy quickstart [--out]` command prints the path and the measured result and
    (with `--out`) persists `quickstart-report.json`. Three new tests.
  - **PV-3 (PRD §5.10) — drift blocks automatic promotion.** Recorded against
    `entelechy-domain::promotion::drift_drill()` (built in the real-domain-study entry
    below): a drift regressing behavior beyond a declared `DriftTolerance` blocks the
    *automatic* promotion path, while a within-tolerance material drift and a recorded
    human override do not.
  - **OL-2 (PRD §17.4) — degraded mode.** A new `entelechy-ops::degraded` module:
    `authorize_fallback` checks a fallback against its *own* capabilities and
    confidentiality tier (authority is never inherited from the primary), a
    `DegradedJournal` records entering/leaving degraded mode with a reason, and
    `registered_evidence`/`excluded_from_registered` keep degraded-mode observations out
    of the evidence a release rests on. `degraded_mode_drill()` returns a
    `DegradedModeDrillReport` proving all three. Four new tests.
  - **DR-2 (PRD §17.3) — erasure/revocation survival and reconcile-first.** The
    `entelechy-artifacts::backup` module now also carries a revocation set and in-flight
    `PendingEffect`s through the `BackupPacket`; `run_restore_drill`/`RestoreReport`
    assert that revocations survive a restore (never resurrected, like erasures) and
    that every in-flight effect whose commit was unknown at backup time is reconciled
    before the restored state is `ready_for_new_work`. Two new tests.
  - **DR-3 (PRD §17.3) — partial loss blocks promotion.**
    `entelechy-artifacts::platform::assess_evidence_state` marks a run/release
    `EvidenceState::Incomplete` when a required artifact did not survive a restore, and
    `entelechy-release::promote` gains a non-override-able `evidence_state` gate
    (`PromotionError::EvidenceIncomplete`). A new cross-crate integration test
    (`entelechy-integration/tests/recovery.rs`) exercises the whole path — back up, lose
    a block, restore the survivors, assess, and confirm the real promotion gate refuses
    even with a recorded override, then passes once the evidence is whole again.
  - **PC-2 / EL-1** are recorded against existing mechanisms (`negotiate_secure` +
    `no_silent_security_downgrade`; the `HoldoutVault` coarse-response / per-query-audit
    / budget tests).

  The `entelechy drill` command now takes `--kind restore|rollback|audit|slo|drift|
  degraded|all` (default `all`): `drift` runs the PV-3 drill and `degraded` the OL-2
  drill, each printing a per-invariant report and (with `--out`) persisting
  `drift-report.json` / `degraded-report.json`; and the `restore` drill now also shows
  the DR-2 revocation-survival and reconcile-first invariants, populated from a real
  revoked release id and an in-flight effect keyed by a genuine `OperationKey`.
  `entelechy requirements --check --phase p4` now prints PASS. 14 new tests.
- **Phase-0 real-domain study (PRD §21, Q1; retires the PV-2 and DP-1 waivers).**
  A new `entelechy-domain` crate replaces the *declared simulator* — which decided a
  task's success by inspecting the design *structure* — with a genuine resettable
  domain. `HelpdeskWorld` is mutable state (a drafted reply with its grounding
  status, a set of resolved ticket parts, a refund ledger); `execute` is an IR
  interpreter that applies each node's *effect* to the world (an `Llm` node drafts an
  ungrounded reply through a real `ModelGateway` call, a `Verify` node grounds it, a
  `Par` committee resolves every part, a forbidden `refund` tool is refused); and
  `resolves` decides success with a programmatic state checker that inspects only the
  final world, never the program — so scoring is no longer circular. On top of that
  domain, `real_study()` produces the two pieces of Phase-0 evidence the TR-1 gate
  still waived:
  - **PV-2 (no silent pooling across material drift).** Every model call's
    `ModelFingerprint` (a content address over the drift-relevant provider-identity
    fields) keys an `EvidenceEpoch`. A material revision drift (a `RevisionModel`
    stamping a new revision behind the same advertised model) opens a *new* epoch
    instead of extending the old one, and `EvidenceLedger::release_evidence()` reads
    only the current epoch. The study shows the drifted model regress (verification
    calibrated to the old revision stops grounding the reply) and proves that pooling
    pre-drift success with post-drift failure would mask the regression — which the
    epoch separation structurally prevents.
  - **DP-1 (one declared deployment profile).** `DeploymentDeclaration` names exactly
    one `DeploymentProfile` and carries a signed `Approval` bound to it — the formal
    single-deployment-profile approval step — revalidated like any approval (a
    declaration whose profile changes reads as stale).

  The run also re-gates the repaired candidate against the baseline on the sealed
  holdout (EV-14) using the real executor, and seals a content-addressed
  `ReproductionPacket` that re-runs to the same resolved counts bit-for-bit.
  `entelechy study --real [--out]` runs it and (with `--out`) persists
  `real-study-report.json` and the two designs. The PV-2 (StudyEvidence, P0) and DP-1
  (HumanApproval, P1) waivers in `phase_gate_evidence()` are replaced with real
  verification records citing the new crate's tests — **no waivers remain** in the
  current evidence set. `requirements --check` still passes the P1 gate; P0 now
  passes with PV-2 verified; and the P4 gap list no longer includes PV-2 or DP-1.
  21 new tests.
- **SLO / error-budget operational drill (PRD §18.1, SL-1).** A new
  `entelechy-release::slo` module mechanizes both halves of SL-1. A
  `ServiceLevelObjective` carries the four things SL-1 requires an L5 SLO to state —
  a measurement window, the population, the statistic plus its target (from which the
  error budget `1 - target` is derived), and the exclusions — and `defects()` fails
  closed on any that is missing or malformed (zero window, empty population, a target
  outside `(0, 1)`, an out-of-range latency percentile, or exclusions never declared),
  so an under-specified objective can never be declared at L5. An `ErrorBudgetAccount`
  accounts only availability/latency *SLO events* against the budget — there is
  deliberately no method to charge a correctness-gate outcome to it — and
  `release_health(correctness, budget)` evaluates the correctness verdict *first and
  independently* of the budget. The `Correctness` verdict is derived from the real
  `PromotionGate` (`assurance.release_eligible()` + `holdout_passed` +
  `negative_goal_clean`), so the drill speaks about the actual correctness gates. The
  new `slo_drill()` returns an `SloDrillReport` (one boolean per invariant) proving:
  a fully-declared L5 SLO is accepted and an under-specified one is rejected with each
  omission named; the error budget equals `1 - target` and spend is tracked over
  observed events; with the budget fully intact a correctness-gate failure still
  blocks (remaining budget never buys back correctness); an exhausted budget with
  clean correctness is only an operational freeze (`BudgetExhausted`), not a
  correctness block; and the real L5 promotion gate refuses a failed correctness gate
  while taking *no* error-budget input, so the only way past is a recorded
  `PromotionOverride` (a human-approved exception), never spending error budget. The
  `entelechy drill` command now takes `--kind restore|rollback|audit|slo|all` (default
  `all`): `slo` runs this drill and with `--out` persists `slo-report.json`. SL-1 is
  first enforced at P4, so its operational-drill verification is recorded in
  `phase_gate_evidence()` now; the current P1 gate is unchanged. Five new tests
  (composite SL-1 invariants, determinism, per-field well-formedness, budget
  arithmetic, correctness/budget independence both ways).
- **Audit / attestation operational drill (PRD §17.2, AU-1/AU-2/AU-3).** A new
  `entelechy-identity::drill` module composes the identity stack into the single
  operational/key drill those three requirements call for: `audit_drill()` returns
  an `AuditDrillReport` with one boolean per invariant. It proves **AU-1** — a
  privileged operation carries an authenticated identity whose credential expiry
  fails closed, authentication never implies authorization (separation of duties
  still refuses a self-approval while admitting a distinct approver), and every
  privileged action lands in a hash-chained, externally-anchored audit log whose
  external anchor detects a full rewrite even when the forged log is internally
  consistent; **AU-2** (key drill) — a signing key rotates (a new key signs and
  validates new writes) and revokes (a revoked key can never sign a new write and
  its existing approval fails closed) *without* mutating the content-addressed
  artifact, whose address is stable across the whole lifecycle; and **AU-3** — a
  security-sensitive time decision on an uncertain clock fails closed for both
  authentication and an expiring approval rather than defaulting to "valid". The
  `entelechy drill` command now takes `--kind restore|rollback|audit|all` (default
  `all`): `audit` runs this drill, prints the per-invariant report, and with `--out`
  persists `audit-report.json`. The AU-1/AU-2/AU-3 operational-drill verification is
  recorded in `phase_gate_evidence()` now (they are first enforced at P4), so the P4
  gate is already covered when the gate advances; the current P1 gate is unchanged.
  Two new drill tests (the composite AU-1/AU-2/AU-3 invariants, determinism).
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
