# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, only when asked.

## Current state (verified)

- HEAD `main` — the SLO / error-budget drill commit, on top of the AU
  audit/attestation drill `e891f94`, the RB rollback drill `9ea4712`, the DR-1
  backup/restore drill `7395d68`, and the closed operate → re-study → release loop
  `4014620`. All committed and pushed.
- **Working tree clean.** The SLO drill is complete: the model
  (`entelechy-release::slo` — `ServiceLevelObjective`, `ErrorBudgetAccount`,
  `Correctness`/`ReleaseHealth`), the drill (`slo_drill`), the operator surface
  (`entelechy drill --kind slo`), the P4 phase-gate record (SL-1), and the docs are
  all committed.
- Verified: `cargo fmt --all --check` clean; `cargo clippy --workspace --all-targets
  -- -D warnings` clean; `cargo clippy -p entelechy-gateway --all-targets --features
  wasm,openai-tls -- -D warnings` clean; `cargo test --workspace -j 2` =
  **302 passed, 0 failed**; `RUSTDOCFLAGS="-D warnings" cargo doc --workspace
  --no-deps` clean. `entelechy drill --kind slo` and `--kind all` run green;
  `entelechy requirements --check` passes the P1 gate, and the P4 gate no longer
  lists SL-1 as a gap. Build under memory pressure with `-j 2` (a cold
  `--workspace` build OOM-kills, exit 137).
- **Phase 1 is complete.** Phase 0 mechanisms + Phase 1–3 primitives are built;
  Phase 4–5 are mostly library primitives, now being operationalized — the
  online-eval loop closes end to end through a governed, human-gated re-study, and
  **all four operational drills are mechanized**: DR-1 (backup/restore, waiver
  retired), RB-1/RB-2 (rollback), AU-1/AU-2/AU-3 (audit/attestation), and SL-1
  (SLO / error budget).

## Done earlier in this run (committed)

- **SLO / error-budget operational drill (PRD §18.1, SL-1).** A new
  `crates/entelechy-release/src/slo.rs` mechanizes both halves of SL-1. A
  `ServiceLevelObjective` states the four things an L5 SLO must declare (window,
  population, statistic + target → error budget, exclusions) and `defects()` fails
  closed on any omission or malformed value; an `ErrorBudgetAccount` accounts only
  availability/latency SLO events (no path to charge a correctness outcome to it);
  `release_health(correctness, budget)` evaluates the `Correctness` verdict — derived
  from the real `PromotionGate` — first and independently of the budget. `slo_drill()`
  returns an `SloDrillReport` proving: a well-formed L5 SLO is accepted and an
  under-specified one rejected (each omission named); budget = 1 - target with spend
  tracked; a correctness failure blocks with the budget fully intact; an exhausted
  budget with clean correctness is only an operational freeze; and the real L5
  promotion gate refuses a failed correctness gate while taking no budget input (only
  a recorded `PromotionOverride` admits it). `entelechy drill --kind slo [--out]`
  persists `slo-report.json`. SL-1 is first enforced at P4, so its record is in
  `phase_gate_evidence()` now (P1 gate unchanged; P4 no longer lists SL-1 as a gap).
  Five new tests.
- **Audit / attestation operational drill (PRD §17.2, AU-1/AU-2/AU-3).** A new
  `crates/entelechy-identity/src/drill.rs` composes the identity stack into the
  single operational/key drill those three requirements call for. `audit_drill()`
  returns an `AuditDrillReport` (one boolean per invariant): **AU-1** — an
  authenticated privileged operation whose credential expiry fails closed,
  authentication never implying authorization (separation of duties refuses a
  self-approval), and a hash-chained, externally-anchored audit log that detects a
  full rewrite; **AU-2** (key drill) — key rotation (a new key signs new writes) and
  revocation (a revoked key never signs again, its approval fails closed) *without*
  mutating the content-addressed artifact (stable address across the lifecycle);
  **AU-3** — an uncertain clock fails closed rather than defaulting to "valid". The
  `entelechy drill` command now takes `--kind restore|rollback|audit|all` (default
  `all`); `--kind audit [--out]` persists `audit-report.json`. AU-1/AU-2/AU-3 are
  first enforced at P4, so their verification records are recorded in
  `phase_gate_evidence()` now (the P1 gate is unchanged; `entelechy requirements
  --check --phase p4` no longer lists AU-1/AU-2/AU-3 as gaps). Two new drill tests
  (composite AU invariants, determinism).
- **Rollback disaster-recovery drill (PRD §14.6, RB-1/RB-2).** A new
  `crates/entelechy-release/src/drill.rs` mechanizes the operational drill those two
  requirements call for. `rollback_drill()` runs the real promote → deploy →
  trigger → rollback → revoke sequence and returns a `RollbackDrillReport`: **RB-2** —
  a production (L4/L5) promotion with no tested rollback target *and* no approved
  no-rollback rationale is refused at the real promotion gate, while a rollback-ready
  bundle passes; **RB-1** — stop authority is separate from promote authority (an
  on-call stopper cannot deploy; a releaser cannot roll back or revoke), the declared
  auto-rollback triggers fire while an undeclared signal does not, a rollback recovers
  the prior known-good release, and emergency revocation is a fail-safe stop.
- **Backup → restore → verify disaster-recovery drill (PRD §17.3, DR-1)** —
  committed in `7395d68`. `crates/entelechy-artifacts/src/backup.rs`: a `BackupPacket`
  over the durable state (objects + owning tenant, event log, lineage, erasure ledger)
  and `run_restore_drill`, which restores *through* serialized bytes and verifies
  hashes (fail-closed on tampering), authorization (tenant firewall re-enforced),
  lineage + events round-trip, and erasure survival (DR-2). `entelechy drill --kind
  restore --out` persists the backup packet + `restore-report.json`. Retired the DR-1
  TR-1 waiver; two waivers remain (PV-2, DP-1).
- **Closed the operate → re-study → release loop (PRD §14.3, §21, OP-5)** —
  committed in `4014620`. `crates/entelechy-cli/src/restudy.rs` turns the online
  loop's opened drift `ReStudy` into a real re-optimization, gated on a sealed
  independent holdout and **never auto-promoted** — it stops at the release gate for
  human approval. `entelechy ops --out` persists the outcome + release decision in
  the hash-chained `audit-log.json`, re-verified by `entelechy audit`.

## Recommended next item

**All four operational drills (DR-1, RB-1/RB-2, AU-1/AU-2/AU-3, SL-1) are done.**
The remaining P4 gaps are no longer drills; they center on the Phase-0 real-domain
study. Natural follow-ons:
1. **Phase-0 study on a real domain (unblocks the last two waivers).** `study` today
   uses a *declared simulator* with no resettable domain, comparator or reproduction
   packet (first gaps-table row). Building a real resettable helpdesk domain would let
   a study produce the no-silent-pooling evidence that retires the **PV-2** waiver and
   the single-deployment-profile approval that retires the **DP-1** waiver — the last
   two TR-1 waivers in `phase_gate_evidence()`.
2. **Exercise the level-2 retrieval path end to end** by adding a retrieval-miss
   failure class to the `study`/`demo` suite (today it only escalates through
   levels 3 and 4), so a run visibly unlocks retrieval.
3. **Deeper re-study fidelity (optional).** The closed loop's repaired candidate is
   a hand-authored typed patch; a richer version would drive the incident's
   regression suite through the real `search`/`design` proposer loop (as `study`
   does) so the re-optimization itself is proposed, not pre-written.
4. **Remaining P4 gaps (requirements --check --phase p4).** PV-3, EL-1, PC-2, DR-2,
   DR-3, OL-2, DX-1 are still unverified at P4 (the gate CI enforces is P1, which
   passes). Several already have real backing code (e.g. DR-2/DR-3 erasure + evidence
   state in `entelechy-artifacts`) and mainly need `phase_gate_evidence()` records
   plus matching tests before the gate advances to P4.

## Top remaining gaps (from the PRD §21 / §17.6 audit)

| Gap | Phase | Note |
|---|---|---|
| Phase-0 study never run on a real domain | P0 | `study` uses a *declared simulator*; no resettable helpdesk / comparator / reproduction packet |
| Online-eval / drift / incident loop | P4 | **done end to end**: loop driven by real `EvalContract` scores, output persisted + audited, and the opened drift re-study now closed through a human-gated `HoldoutVault` release (`entelechy-ops::online` + `entelechy-cli::restudy` + `entelechy ops [--out]`, `entelechy audit`). Optional: drive the re-study's repair through the real proposer loop (next-item §4) |
| Operational drills | P4 | **all four done**: DR-1 backup/restore (`entelechy-artifacts::backup` + `drill --kind restore`, waiver retired); RB-1/RB-2 rollback (`entelechy-release::drill` + `drill --kind rollback`); AU-1/AU-2/AU-3 audit (`entelechy-identity::drill` + `drill --kind audit`); SL-1 SLO/error budget (`entelechy-release::slo` + `drill --kind slo`). All carry P4 `phase_gate_evidence()` records |
| Last two TR-1 waivers (PV-2, DP-1) | P4 | PV-2 needs a real-domain study's no-silent-pooling evidence; DP-1 needs a formal single-deployment-profile approval step — both depend on the Phase-0 real-domain study |
| Shadow/canary rollout, web console, SDKs | P4/P3 | not implemented |
| Multi-tenancy, plugin registry, learned priors, distillation | P5 | mostly not implemented (cluster execution is done) |

## How to work here (see CLAUDE.md for the durable invariants)

- Build under memory pressure with `-j 2` — a cold `--workspace` build OOM-kills
  (exit 137) on this machine.
- `gh` is NOT authenticated: push over the SSH remote; for PR/CI status use
  unauthenticated `curl https://api.github.com/repos/mi7plus/entelechy/...`.
- Windows + git-bash; watch for stray 0-byte files from shell quirks — run
  `git status` before every commit.
- Conventions: each crate's `//!` cites its PRD section; commit messages reference
  requirement IDs (OC-*, IR-I*, EV-*, RK-*, AC-*, AU-*, …); heavy engines stay behind
  features (`wasm`, `openai`, `openai-tls`).
- The TR-1 phase gate is `entelechy requirements --check`. A new requirement or a
  phase advance needs matching evidence/waivers in
  `entelechy_contracts::phase_gate_evidence()` (and bumping `CURRENT_GATE_PHASE`).

The CLI surface: `init, objective, capability, eval, design, study, trace, replay,
counterfactual, diff, assure, release, serve, committee, audit, demo, ops,
drill [--kind restore|rollback|audit|slo|all], requirements [--check]` (+ `infer`
under `--features openai`).
