# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, only when asked.

## Current state (verified)

- HEAD `main` — the online-eval-loop commit (this change), tree clean.
- Full gate green at HEAD: `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -- -D warnings` (plus gateway `--features wasm,openai-tls`),
  `cargo test --workspace` (**278 passed**), `RUSTDOCFLAGS="-D warnings" cargo doc
  --workspace --no-deps`. Build under memory pressure with `-j 2`.
- **Phase 1 is complete.** Phase 0 mechanisms + Phase 1–3 primitives are built;
  Phase 4–5 are mostly library primitives, now beginning to be operationalized
  (the online-eval loop below is the first running P4 loop).

## Done since the last handoff

- **Online-eval / drift / incident loop (PRD §15, §21.4).** A running
  `OnlineEvalLoop` (`entelechy-ops::online`) composes the crate's stateless
  primitives into the P4 "measure and operate in production" story. It consumes a
  stream of `RunObservation`s + user `Feedback` and emits a typed, auditable
  `OpsEvent` log: sample at a bounded rate (OP-2); per drift signal keep a frozen
  baseline + rolling recent window and re-run `detect_drift` (OP-3); on drift — or
  a satisfaction breach — capture an `Incident` from the trace, make it permanent
  regression coverage (OP-4, EV-12), and open a *bounded* `ReStudy` that waits at
  the release gate and never auto-promotes (OP-5, §14.3); charge every re-study to
  a `QuotaLedger` and emit a hard-stop event once the budget is spent (OP-7,
  §17.4). Drift latches (one re-study per regime, not per run); the loop is
  deterministic and replayable. New `entelechy ops [--out <dir>]` CLI command
  drives a bundled run+feedback scenario and prints/persists the event trail.
- **Retrieval IR node + level-2 search unlock (PRD 11.4 level 2, §12).** A new
  `Retrieval` node (`entelechy-ir`): query template + tier + top-k, statically a
  read (no capability/authority; output is tainted and carries forward retrieved
  confidentiality labels — IR-I9, IR-I3). The runtime executes it against
  `TieredMemory::retrieve` (deterministic lexical-overlap, stable score-desc/
  key-asc order); it records **no journal event** and replays/counterfactuals
  reproduce it exactly (RK-2). `Engine::seed_memory` stages the knowledge base.
  The design layer gains `EditOp::AddRetrieval` (prepend to the root `Seq`), and
  the search proposer now emits it at **level 2** for `knowledge.retrieval-miss`
  — closing the one gap where that class was eligible but had no operator. The
  compiled backend reports `Retrieval` as unsupported (the interpreter is its
  reference executor for the Q10 conformance gate).

## Recommended next item: connect the online-eval loop to real scores / drills

The `OnlineEvalLoop` is now the running P4 driver, but it consumes *pre-computed*
metric streams and keeps incidents/re-studies in memory. Two natural follow-ons:
1. **Score real runs into the loop (PRD §15, §9).** Feed it metrics derived from
   executing/evaluating production runs against the live `EvalContract`
   (`entelechy-eval`) instead of hand-supplied values, and persist the emitted
   incidents/re-studies + audit log (as `study --out` does), so an operator has a
   durable record. Keep `entelechy-ops` a leaf: the wiring belongs in the CLI or a
   new thin crate, not as an `entelechy-eval` dependency of ops.
2. **Operational drills (RB/DR/AU/PV/SL, P4).** Not implemented; mechanizing them
   also retires the three TR-1 waivers (PV-2, DR-1, DP-1) — see the gaps table.

Smaller alternative follow-on: exercise the level-2 retrieval path end to end by
adding a retrieval-miss failure class to the `study`/`demo` suite (today it only
escalates through levels 3 and 4), so a run visibly unlocks retrieval.

## Top remaining gaps (from the PRD §21 / §17.6 audit)

| Gap | Phase | Note |
|---|---|---|
| Phase-0 study never run on a real domain | P0 | `study` uses a *declared simulator*; no resettable helpdesk / comparator / reproduction packet |
| Online-eval / drift / incident loop | P4 | **loop built** (`entelechy-ops::online` + `entelechy ops` demo); still open: feed real `EvalContract` scores and persist incidents/re-studies (next-item §1) |
| Operational drills (RB/DR/AU/PV/SL) | P4 | not implemented; retiring these also clears the TR-1 waivers (PV-2, DR-1, DP-1) |
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
  requirement IDs (OC-*, IR-I*, EV-*, RK-*, AC-*, …); heavy engines stay behind
  features (`wasm`, `openai`, `openai-tls`).
- The TR-1 phase gate is `entelechy requirements --check`. A new requirement or a
  phase advance needs matching evidence/waivers in
  `entelechy_contracts::phase_gate_evidence()` (and bumping `CURRENT_GATE_PHASE`).

The CLI surface: `init, objective, capability, eval, design, study, trace, replay,
counterfactual, diff, assure, release, serve, committee, audit, demo, ops,
requirements [--check]` (+ `infer` under `--features openai`).
