# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, only when asked.

## Current state (verified)

- HEAD `d96988c`, branch `main`, tree clean.
- Full gate green at HEAD: `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -- -D warnings` (plus gateway `--features wasm,openai-tls`),
  `cargo test --workspace` (**264 passed**), `RUSTDOCFLAGS="-D warnings" cargo doc
  --workspace --no-deps`. Build under memory pressure with `-j 2`.
- **Phase 1 is complete.** Phase 0 mechanisms + Phase 1–3 primitives are built;
  Phase 4–5 are mostly library primitives, not operationalized.

## Done since the last handoff

- **1.0 contract freeze (Phase-1 exit).** `docs/contracts-1.0.md` + the
  machine-readable `entelechy_contracts::FROZEN_CONTRACTS` freeze six contracts
  (canonical serialization, Design IR, journal, model-request, failure ontology,
  protocol) with explicit version constants (`JOURNAL_FORMAT_VERSION`,
  `NATIVE_REQUEST_SCHEMA_VERSION`, `CURRENT_PROTOCOL_VERSION` were added). A
  version-bump gate (`entelechy-cli/tests/contract_freeze.rs`) ties each frozen
  version to its owning constant; the golden tests pin the bytes.
- **TR-1 traceability check in CI.** `entelechy requirements --check` +
  `entelechy_contracts::phase_gate_evidence()` + a `traceability` CI job fail on any
  requirement first enforced at/before the current gate phase (P1) with no
  verification record and no active waiver. Three study/drill/approval requirements
  carry scoped waivers (PV-2, DR-1, DP-1) that **expire 2027-07-01** and must be
  retired by real evidence (see gaps below).

## Recommended next item: Retrieval IR node (P3)

Started this session. Memory tiers (`entelechy-runtime` `MemoryTier`/`TieredMemory`)
and retrieval *metrics* (`entelechy-eval`) exist, but there is no Retrieval IR node
and the search proposer cannot yet unlock retrieval. Shape:
- Add a `Retrieval` node to the IR (`entelechy-ir`): query template + source/tier +
  top-k, with static invariants (it is a read; honors confidentiality/taint labels).
- Execute it in the runtime against a memory/knowledge source, journaling the
  retrieval as an effect so replay is exact.
- Wire the search proposer (`entelechy-search`) to unlock it on a
  `knowledge.retrieval-miss` failure class, within the Architecture Explorer's
  ceiling/budget/authority rules (Q4).

## Top remaining gaps (from the PRD §21 / §17.6 audit)

| Gap | Phase | Note |
|---|---|---|
| Retrieval IR node + proposer unlock | P3 | recommended next (in progress) |
| Phase-0 study never run on a real domain | P0 | `study` uses a *declared simulator*; no resettable helpdesk / comparator / reproduction packet |
| Online-eval / drift / incident loops | P4 | `entelechy-ops` has primitives (`sample_for_eval`, `FeedbackStore`, `DriftKind`); no running loop |
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
counterfactual, diff, assure, release, serve, committee, audit, demo,
requirements [--check]` (+ `infer` under `--features openai`).
