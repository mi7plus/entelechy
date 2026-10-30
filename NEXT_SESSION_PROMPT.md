# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, only when asked.

## Current state (verified)

- HEAD `main` — the real-EvalContract-scores commit (this change), tree clean.
- Full gate green at HEAD: `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -- -D warnings` (plus gateway `--features wasm,openai-tls`),
  `cargo test --workspace` (**281 passed**), `RUSTDOCFLAGS="-D warnings" cargo doc
  --workspace --no-deps`. Build under memory pressure with `-j 2`.
- **Phase 1 is complete.** Phase 0 mechanisms + Phase 1–3 primitives are built;
  Phase 4–5 are mostly library primitives, now beginning to be operationalized
  (the online-eval loop below is the first running P4 loop).

## Done since the last handoff

- **Online-eval loop now driven by real EvalContract scores (PRD §15, §9, §21.4).**
  `entelechy ops` no longer feeds the `OnlineEvalLoop` hand-supplied numbers. It
  executes a deployed single-agent triage design over a stream of production
  tickets through the real runtime, scores each run's output against a live
  `EvalContract` with programmatic checkers (EV-4), and feeds the measured
  `task_success` as the drift signal (user feedback is likewise derived from each
  run's real resolution). The agent resolves reply tickets but refuses a refund at
  the gate (a structural negative goal), so a refund surge drops the measured
  `task_success`; the loop detects that drift *from the scores*, opens an incident,
  captures regression coverage (EV-12) and queues a bounded re-study (OP-5). The
  command prints the per-run scores and the decision trail; `--out` persists the
  event log. The scoring wiring lives in the CLI, keeping `entelechy-ops` a leaf;
  three CLI tests lock in the real-score-driven drift, the design's static validity
  and run determinism.

## Recommended next item: persist the loop's output, or operational drills

The loop now runs on real `EvalContract` scores, but its incidents/re-studies live
only in memory. Two natural follow-ons:
1. **Persist the online loop's output (PRD §15, 17.2).** Write the emitted
   incidents, regression tasks and re-studies — plus a hash-chained audit log, as
   `study --out` does — so operators have a durable, tamper-evident record; and
   feed a captured re-study back into the `study` loop so an incident actually
   drives a re-optimization (closing the operate → re-study → release cycle).
2. **Operational drills (RB/DR/AU/PV/SL, P4).** Not implemented; mechanizing them
   retires the three TR-1 waivers (PV-2, DR-1, DP-1) — see the gaps table.

Smaller alternative follow-on: exercise the level-2 retrieval path end to end by
adding a retrieval-miss failure class to the `study`/`demo` suite (today it only
escalates through levels 3 and 4), so a run visibly unlocks retrieval.

## Top remaining gaps (from the PRD §21 / §17.6 audit)

| Gap | Phase | Note |
|---|---|---|
| Phase-0 study never run on a real domain | P0 | `study` uses a *declared simulator*; no resettable helpdesk / comparator / reproduction packet |
| Online-eval / drift / incident loop | P4 | **loop built + driven by real `EvalContract` scores** (`entelechy-ops::online` + `entelechy ops`); still open: persist incidents/re-studies + audit log, and feed a re-study back into `study` (next-item §1) |
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
