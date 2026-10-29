# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, only when asked.

## Current state (verified)

- HEAD `6301366`, branch `main`, tree clean.
- Full gate green at HEAD: `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace` (**251 passed**),
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`.
- Phase 0 mechanisms + Phase 1–3 primitives are built; Phase 4–5 are mostly
  library primitives, not operationalized. See the gap table below.

## Recommended next item: formalize the 1.0 contract freeze (Phase-1 exit)

The clearest bounded deliverable. Golden byte-tests already pin canonical-JSON and
journal formats (`crates/entelechy-artifacts/tests/golden_format.rs`,
`crates/entelechy-runtime/tests/golden_journal.rs`) and `CANONICAL_SERIALIZATION_VERSION`
exists. What's missing is the *formalization*: a documented frozen-schema policy
(IR, artifacts, journal, task/checker interfaces, AuthorityEnvelope) with explicit
version constants and a compatibility/version-bump gate. Suggested approach:
- Add a `docs/contracts-1.0.md` (or similar) listing each frozen schema + its
  version and the compatibility promise.
- Add schema-version constants where missing and a test/gate that fails if a
  frozen byte format changes without a version bump (extend the golden tests).

## Top remaining gaps (from the PRD §21 / §17.6 audit)

| Gap | Phase | Note |
|---|---|---|
| 1.0 contract freeze (formalization) | P1 | recommended next |
| TR-1 traceability check not in CI | P1 | `entelechy-contracts::TraceabilityIndex` exists; wire into `.github/workflows/ci.yml` |
| Phase-0 study never run on a real domain | P0 | `study` uses a *declared simulator*; no resettable helpdesk / comparator / reproduction packet |
| Retrieval IR node + level-2 proposer unlock | P3 | memory tiers + retrieval *metrics* exist; no Retrieval node |
| Online-eval / drift / incident loops | P4 | `entelechy-ops` has primitives (`sample_for_eval`, `FeedbackStore`, `DriftKind`); no running loop |
| Shadow/canary rollout, web console, SDKs | P4/P3 | not implemented |
| Operational drills (RB/DR/AU/PV/SL) | P4 | not implemented |
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

The CLI surface: `init, objective, capability, eval, design, study, trace, replay,
counterfactual, diff, assure, release, serve, committee, audit, demo, requirements`
(+ `infer` under `--features openai`).
