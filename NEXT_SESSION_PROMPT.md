# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, only when asked.

## Current state (verified)

- HEAD `main` — the online-ops-persistence commit `e1844a8`. The **closed
  operate → re-study → release loop** (this change) is complete and gate-green in
  the working tree, committed together with this handoff once the user approves.
- Full gate green (working tree): `cargo fmt --all --check`, `cargo clippy
  --workspace --all-targets -- -D warnings` (plus gateway `--features
  wasm,openai-tls`), `cargo test --workspace` (**288 passed**), `RUSTDOCFLAGS="-D
  warnings" cargo doc --workspace --no-deps`. Build under memory pressure with
  `-j 2`.
- **Phase 1 is complete.** Phase 0 mechanisms + Phase 1–3 primitives are built;
  Phase 4–5 are mostly library primitives, now being operationalized — the
  online-eval loop is the first running P4 loop and it now closes end to end
  through a governed, human-gated re-study.

## Done since the last handoff

- **Closed the operate → re-study → release loop (PRD §14.3, §21, OP-5).** A new
  `crates/entelechy-cli/src/restudy.rs` turns the online loop's opened drift
  `ReStudy` into a real re-optimization. It builds a repaired candidate as a
  minimal typed patch — the deployed design with its refund-refusing `Gate` removed,
  so refund tickets get a safe drafted escalation instead of being refused — with
  the authority unchanged (`refund` still forbidden/ungranted), so the structural
  negative goal is fully preserved. The candidate fixes the incident-derived
  regression class (`Provenance::Incident`, EV-12; scored directly, never counted as
  independent holdout per §9.5) 0/8 → 8/8, then is gated on a sealed independent
  holdout through the assurance-only `HoldoutVault` release gate (EV-14, §9.6),
  which passes on the measured ~50pp improvement — yet the candidate is **never
  auto-promoted** (`ReStudy::auto_promotes()` is always false); it stops at the
  release gate for human approval. `entelechy ops` runs the re-study and prints the
  outcome; `--out` persists it in `ops-report.json` (regression fix, coarse gate
  verdict, `promoted: false`) and records the release-gate decision as the final
  entry in the hash-chained `audit-log.json`, re-verified by `entelechy audit`.
  `ops.rs` grew a reusable `execute_on`/`resolves` pair so the loop and the
  re-study gate score through the same real runtime + `EvalContract` checkers.
  Five new `restudy` tests (plus two strengthened `ops` tests) cover IR-validity +
  negative-goal preservation, both-class resolution, the gated-but-unpromoted
  outcome, the end-to-end demo path, determinism, and the persisted round-trip.

## Recommended next item

The measure → operate → re-study → release cycle is now closed end to end. Natural
follow-ons:
1. **Operational drills (RB/DR/AU/PV/SL, P4).** Not implemented; mechanizing them
   retires the three TR-1 waivers (PV-2, DR-1, DP-1) — see the gaps table.
2. **Exercise the level-2 retrieval path end to end** by adding a retrieval-miss
   failure class to the `study`/`demo` suite (today it only escalates through
   levels 3 and 4), so a run visibly unlocks retrieval.
3. **Deeper re-study fidelity (optional).** The closed loop's repaired candidate is
   a hand-authored typed patch; a richer version would drive the incident's
   regression suite through the real `search`/`design` proposer loop (as `study`
   does) so the re-optimization itself is proposed, not pre-written.

## Top remaining gaps (from the PRD §21 / §17.6 audit)

| Gap | Phase | Note |
|---|---|---|
| Phase-0 study never run on a real domain | P0 | `study` uses a *declared simulator*; no resettable helpdesk / comparator / reproduction packet |
| Online-eval / drift / incident loop | P4 | **done end to end**: loop driven by real `EvalContract` scores, output persisted + audited, and the opened drift re-study now closed through a human-gated `HoldoutVault` release (`entelechy-ops::online` + `entelechy-cli::restudy` + `entelechy ops [--out]`, `entelechy audit`). Optional: drive the re-study's repair through the real proposer loop (next-item §3) |
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
