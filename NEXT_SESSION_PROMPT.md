# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, only when asked.

## Current state (verified)

- **The Phase-0 real-domain study is complete and the full gate is green.** It is
  in the working tree pending commit (HEAD `main` is still the SLO drill commit
  `76313c7`, on top of the AU drill `e891f94`, the RB rollback drill `9ea4712`, the
  DR-1 restore drill `7395d68`, and the closed operate → re-study → release loop
  `4014620`). Once committed this becomes the new HEAD.
- Verified on the current tree: `cargo fmt --all --check` clean; `cargo clippy
  --workspace --all-targets -- -D warnings` clean; `cargo clippy -p
  entelechy-gateway --all-targets --features wasm,openai-tls -- -D warnings` clean;
  `cargo test --workspace -j 2` = **323 passed, 0 failed** (302 + 21 new);
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` clean. `entelechy
  study --real` prints `SATISFIED`; `entelechy requirements --check` passes the P1
  gate, P0 passes with PV-2 verified, and the P4 gap list no longer includes PV-2 or
  DP-1. Build under memory pressure with `-j 2` (a cold `--workspace` build
  OOM-kills, exit 137).
- **No TR-1 waivers remain.** Every cross-cutting requirement enforced through the
  current P1 gate now has a real verification record. The waiver *mechanism* stays
  in `phase_gate_evidence()` for future scoped gaps, but the current evidence set
  uses none.
- **Phase 1 is complete.** Phase 0 mechanisms + Phase 1–3 primitives are built;
  Phase 4–5 are mostly library primitives, now being operationalized. All four
  operational drills are mechanized (DR-1, RB-1/RB-2, AU-1/AU-2/AU-3, SL-1), the
  online-eval loop closes end to end through a governed human-gated re-study, and
  the Phase-0 study now runs on a **real resettable domain**.

## Done this run (built + verified, pending commit)

- **Phase-0 real-domain study — new `entelechy-domain` crate (PRD §21, Q1).**
  Replaces the *declared simulator* (which scored a task by inspecting the design
  *structure*) with a genuine domain:
  - `world.rs` — `HelpdeskWorld`: resettable mutable state (a drafted `Reply` with a
    `grounded` flag, resolved ticket parts, a refund ledger) with programmatic state
    checkers (`resolved()`, `negative_goal_held()`). Grounding is calibrated to a
    model revision (`CALIBRATED_REVISION = "mock-r1"`), which is what makes a silent
    drift break it.
  - `exec.rs` — `execute`/`resolves`/`compare`: an IR interpreter that applies each
    node's *effect* (an `Llm` node drafts an ungrounded reply through a real
    `ModelGateway` call; `Verify` grounds it; a `Par` committee of `resolve_part`
    workers resolves every part; a forbidden `refund` tool is refused by the
    authority envelope and the world). Success is read from world state, not the
    program. `ComparisonReport` is the baseline-vs-candidate comparator.
  - `epoch.rs` — `ModelFingerprint` (content address over the drift-relevant
    provider-identity fields), `EvidenceEpoch`, `EvidenceLedger` (a new fingerprint
    opens a new epoch; `release_evidence()` reads only the current one — PV-2), and
    `RevisionModel` (a `MockModel` that stamps a chosen revision to model drift).
  - `study.rs` — `real_study()` → `RealStudyReport` (one bool per invariant): the
    comparator, a holdout gate (EV-14) over the real executor, the PV-2
    no-silent-pooling proof across a material drift, a DP-1 `DeploymentDeclaration`
    (exactly one `DeploymentProfile` + a signed approval bound to it), and a
    content-addressed `ReproductionPacket` that re-runs to identical counts.
  - CLI: `entelechy study --real [--out]` runs it, prints the invariant narrative,
    and (with `--out`) persists `real-study-report.json` + the two designs.
- **Retired the PV-2 and DP-1 waivers.** `entelechy_contracts::phase_gate_evidence()`
  now records PV-2 (StudyEvidence, P0) and DP-1 (HumanApproval, P1) against the new
  crate's tests instead of waiving them. The two waiver blocks are gone; no waivers
  remain. Gate docs and `WAIVER_REVIEW_DEADLINE`'s doc updated accordingly.

## Recommended next item

The last two TR-1 waivers are gone and all four operational drills are mechanized.
Natural follow-ons, roughly in order of leverage:

1. **Advance the TR-1 gate toward P4.** `requirements --check --phase p4` lists 7
   remaining gaps: PV-3, EL-1, PC-2, DR-2, DR-3, OL-2, DX-1. Several already have
   real backing code — DR-2/DR-3 (erasure survival + `EvidenceState::Incomplete`
   blocking promotion in `entelechy-artifacts`), PC-2 (no-silent-downgrade in
   `entelechy-protocol`), OL-2 (fallback authorization / degraded-mode journaling).
   Each mainly needs a `phase_gate_evidence()` record plus a matching test; once the
   P2/P4 requirements are all covered, bump `CURRENT_GATE_PHASE`.
2. **Unify the two studies / drive the repair through the real proposer loop.** The
   simulated `study` still scores by structure and the re-study repair
   (`entelechy-cli::restudy`) is a hand-authored typed patch. Fold the new real
   executor in as the scoring function (so the proposer loop in `search`/`design`
   optimizes against executed state), and/or point the re-study's repair at the real
   `search`/`design` proposer instead of a pre-written patch.
3. **Exercise the level-2 retrieval path end to end** by adding a retrieval-miss
   failure class to the study/demo suite (today it escalates only through levels 3
   and 4), so a run visibly unlocks retrieval.
4. **Shadow/canary rollout, web console, SDKs (P4/P3)** — not implemented.

## Top remaining gaps (from the PRD §21 / §17.6 audit)

| Gap | Phase | Note |
|---|---|---|
| Phase-0 study on a real domain | P0 | **done**: `entelechy-domain` executes designs against a resettable `HelpdeskWorld` with programmatic state checkers; comparator + reproduction packet; `entelechy study --real`. Retired the PV-2 and DP-1 waivers |
| Online-eval / drift / incident loop | P4 | **done end to end**: loop driven by real `EvalContract` scores, output persisted + audited, opened drift re-study closed through a human-gated `HoldoutVault` release. Optional: drive the re-study repair through the real proposer loop (next-item §2) |
| Operational drills | P4 | **all four done**: DR-1 restore, RB-1/RB-2 rollback, AU-1/AU-2/AU-3 audit, SL-1 SLO/error budget. All carry P4 `phase_gate_evidence()` records |
| Remaining P4 requirements | P4 | PV-3, EL-1, PC-2, DR-2, DR-3, OL-2, DX-1 still unverified at P4 (the gate CI enforces is P1, which passes). Several have real backing code; need records + tests before the gate advances (next-item §1) |
| Shadow/canary rollout, web console, SDKs | P4/P3 | not implemented |
| Multi-tenancy, plugin registry, learned priors, distillation | P5 | mostly not implemented (cluster execution is done) |

## How to work here (see CLAUDE.md for the durable invariants)

- Build under memory pressure with `-j 2` — a cold `--workspace` build OOM-kills
  (exit 137) on this machine.
- `gh` is NOT authenticated: push over the SSH remote; for PR/CI status use
  unauthenticated `curl https://api.github.com/repos/mi7plus/entelechy/...`.
- Windows + git-bash; watch for stray 0-byte files from shell quirks — run
  `git status` before every commit (three stray 0-byte files appeared this run and
  were removed).
- Conventions: each crate's `//!` cites its PRD section; commit messages reference
  requirement IDs (OC-*, IR-I*, EV-*, RK-*, AC-*, PV-*, DP-*, AU-*, …); heavy
  engines stay behind features (`wasm`, `openai`, `openai-tls`).
- The TR-1 phase gate is `entelechy requirements --check`. A new requirement or a
  phase advance needs matching evidence/waivers in
  `entelechy_contracts::phase_gate_evidence()` (and bumping `CURRENT_GATE_PHASE`).

The CLI surface: `init, objective, capability, eval, design, study [--real] [--out],
trace, replay, counterfactual, diff, assure, release, serve, committee, audit, demo,
ops, drill [--kind restore|rollback|audit|slo|all], requirements [--check]` (+ `infer`
under `--features openai`).
