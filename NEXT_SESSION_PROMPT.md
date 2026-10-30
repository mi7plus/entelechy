# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, only when asked.

## Current state (verified)

- HEAD `main` — the online-ops-persistence commit (this change), tree clean.
- Full gate green at HEAD: `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -- -D warnings` (plus gateway `--features wasm,openai-tls`),
  `cargo test --workspace` (**283 passed**), `RUSTDOCFLAGS="-D warnings" cargo doc
  --workspace --no-deps`. Build under memory pressure with `-j 2`.
- **Phase 1 is complete.** Phase 0 mechanisms + Phase 1–3 primitives are built;
  Phase 4–5 are mostly library primitives, now beginning to be operationalized
  (the online-eval loop below is the first running P4 loop).

## Done since the last handoff

- **Online-ops output is now persisted and tamper-evident (PRD §15, 17.2).**
  `entelechy ops --out <dir>` writes the loop's governed output the way
  `study --out` does: the full event log (`ops-log.json`), a structured
  `ops-report.json` (the incidents with their trace refs, the regression tasks and
  the re-studies, plus a commitment to the audit chain head), and a hash-chained
  `audit-log.json` of every governed decision. `entelechy audit <dir>` was
  generalized to re-verify the chain and the ops report's `audit_head` (it already
  did so for `study-report.json`), so a durable, tamper-evident operator record now
  exists for an online run. Two CLI tests cover the audit chain and the
  round-trip (write → re-read → verify); the loop itself still runs on the real
  `EvalContract` scores wired in the previous change.

## Recommended next item: close the operate → re-study → release loop

The online loop now detects drift from real scores and persists typed incidents /
re-studies, but a captured `ReStudy` is still inert. Natural follow-ons:
1. **Feed a captured re-study back into `study` (PRD §14.3, 21).** Turn an opened
   `ReStudy` (and the `RegressionTask` from its incident) into an actual
   re-optimization: seed a `study` run from the incident's failing traces, so an
   operate-time regression drives a governed re-study whose candidate still stops
   at the release gate (never auto-promoted). That closes the measure → operate →
   re-study → release cycle end to end.
2. **Operational drills (RB/DR/AU/PV/SL, P4).** Not implemented; mechanizing them
   retires the three TR-1 waivers (PV-2, DR-1, DP-1) — see the gaps table.

Smaller alternative follow-on: exercise the level-2 retrieval path end to end by
adding a retrieval-miss failure class to the `study`/`demo` suite (today it only
escalates through levels 3 and 4), so a run visibly unlocks retrieval.

## Top remaining gaps (from the PRD §21 / §17.6 audit)

| Gap | Phase | Note |
|---|---|---|
| Phase-0 study never run on a real domain | P0 | `study` uses a *declared simulator*; no resettable helpdesk / comparator / reproduction packet |
| Online-eval / drift / incident loop | P4 | **loop built, driven by real `EvalContract` scores, output persisted + audited** (`entelechy-ops::online` + `entelechy ops [--out]`, `entelechy audit`); still open: feed a captured re-study back into `study` (next-item §1) |
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
