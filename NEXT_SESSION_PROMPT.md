# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, only when asked.

## Current state (verified)

- HEAD `main` — the DR-1 drill commit `7395d68` ("Mechanize the DR-1 backup/restore
  drill and retire its waiver"). The **RB rollback drill** (this change) is complete
  and gate-green in the working tree, to be committed together with this handoff once
  the user approves.
- Full gate green (working tree): `cargo fmt --all --check`, `cargo clippy
  --workspace --all-targets -- -D warnings` (plus gateway `--features
  wasm,openai-tls`), `cargo test --workspace` (**295 passed**), `RUSTDOCFLAGS="-D
  warnings" cargo doc --workspace --no-deps`. Build under memory pressure with
  `-j 2`.
- **Phase 1 is complete.** Phase 0 mechanisms + Phase 1–3 primitives are built;
  Phase 4–5 are mostly library primitives, now being operationalized — the
  online-eval loop closes end to end through a governed, human-gated re-study, and
  two operational drills are mechanized: DR-1 (backup/restore, waiver retired) and
  RB-1/RB-2 (rollback; P4, verification records recorded ahead of the P4 gate).

## Done since the last handoff

- **Rollback disaster-recovery drill (PRD §14.6, RB-1/RB-2).** A new
  `crates/entelechy-release/src/drill.rs` mechanizes the operational drill those two
  requirements call for. `rollback_drill()` runs the real promote → deploy → trigger
  → rollback → revoke sequence and returns a `RollbackDrillReport`: **RB-2** — a
  production (L4/L5) promotion with no tested rollback target *and* no approved
  no-rollback rationale is refused at the real promotion gate, while a rollback-ready
  bundle passes; **RB-1** — stop authority is separate from promote authority (an
  on-call stopper cannot deploy; a releaser cannot roll back or revoke), the declared
  auto-rollback triggers fire while an undeclared signal does not, a rollback recovers
  the prior known-good release, and emergency revocation is a fail-safe stop. The
  `entelechy drill` command now takes `--kind restore|rollback|all` (default `all`);
  `--out` persists `rollback-report.json`. RB-1/RB-2 are first enforced at P4, so
  their operational-drill verification records are recorded in `phase_gate_evidence()`
  now (the P1 gate is unchanged; `entelechy requirements --check --phase p4` no longer
  lists RB-1/RB-2 as gaps). Two new drill tests (composite RB invariants, determinism).
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

The measure → operate → re-study → release cycle is closed, and the first
operational drills DR-1 (backup/restore) and RB-1/RB-2 (rollback) are mechanized.
Natural follow-ons:
1. **More operational drills (AU/SL, P4).** DR-1 and RB are now done; the remaining
   drills are audit/attestation (AU) and SLO (SL). Mechanizing the PV-2 / DP-1 gaps
   (study-on-a-real-domain evidence; a formal deployment-profile approval step) would
   retire the last two TR-1 waivers — see the gaps table.
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
| Operational drills (AU/SL) | P4 | DR-1 backup/restore **done** (`entelechy-artifacts::backup` + `entelechy drill --kind restore`, DR-1 waiver retired) and RB-1/RB-2 rollback **done** (`entelechy-release::drill` + `entelechy drill --kind rollback`, P4 records recorded); audit/attestation (AU) and SLO (SL) drills not yet mechanized |
| Last two TR-1 waivers (PV-2, DP-1) | P4 | PV-2 needs a real-domain study's no-silent-pooling evidence; DP-1 needs a formal single-deployment-profile approval step |
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
drill [--kind restore|rollback|all], requirements [--check]` (+ `infer` under
`--features openai`).
