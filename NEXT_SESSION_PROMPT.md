# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, only when asked.

## Current state (verified)

- HEAD `main` — the closed-loop commit `4014620` ("Close the operate → re-study →
  release loop"). The **DR-1 backup/restore drill** (this change) is complete and
  gate-green in the working tree, to be committed together with this handoff once
  the user approves.
- Full gate green (working tree): `cargo fmt --all --check`, `cargo clippy
  --workspace --all-targets -- -D warnings` (plus gateway `--features
  wasm,openai-tls`), `cargo test --workspace` (**293 passed**), `RUSTDOCFLAGS="-D
  warnings" cargo doc --workspace --no-deps`. Build under memory pressure with
  `-j 2`.
- **Phase 1 is complete.** Phase 0 mechanisms + Phase 1–3 primitives are built;
  Phase 4–5 are mostly library primitives, now being operationalized — the
  online-eval loop closes end to end through a governed, human-gated re-study, and
  the first operational drill (DR-1 backup/restore) is now mechanized and retires
  its TR-1 waiver.

## Done since the last handoff

- **Backup → restore → verify disaster-recovery drill (PRD §17.3, DR-1).** A new
  `crates/entelechy-artifacts/src/backup.rs` mechanizes DR-1 — "backup and restore
  preserve hashes, lineage and authorization metadata." A `BackupPacket` captures
  the durable state (content-addressed objects with their owning tenant, the
  append-only event log, the lineage edge graph and the erasure ledger) and
  serializes to cold-storage bytes; `run_restore_drill` restores *through* those
  bytes and verifies each invariant: every object re-hashes to its address (a
  tampered backup fails closed with `RestoreError::IntegrityFailure`, never loaded),
  each object keeps its owning tenant so the owner reads it and every other tenant
  is refused (PRD §17.2), lineage + events round-trip unchanged, and the erasure
  ledger is replayed so crypto-shredded data never reappears (DR-2). Privileged
  `InMemoryStore::export` and `InMemoryMetadataStore::export`/`import` back the
  whole-store read (distinct from `BlobStore::get`, where a hash is never authority).
  A new `entelechy drill [--out <dir>]` command builds a two-tenant durable-state
  slice (signed objective → design → semantics-preserving migration → release under
  `local`; an isolated design + a crypto-shredded subject under `tenant-b`), runs
  the drill, prints the per-invariant `RestoreReport`, proves fail-closed by
  rejecting a tampered backup, and `--out` persists the backup packet +
  `restore-report.json` as an auditable record. Five new tests (happy path,
  corruption rejection, re-enforced tenant firewall, erasure survival, determinism).
  **Retires the DR-1 TR-1 waiver**: `phase_gate_evidence()` now carries a DR-1
  verification record; two waivers remain (PV-2, DP-1).
- **Closed the operate → re-study → release loop (PRD §14.3, §21, OP-5)** —
  committed in `4014620`. `crates/entelechy-cli/src/restudy.rs` turns the online
  loop's opened drift `ReStudy` into a real re-optimization: a minimal typed patch
  (the deployed design with its refund-refusing `Gate` removed, authority unchanged
  so `refund` stays forbidden), fixing the incident-derived regression class
  (`Provenance::Incident`, EV-12; §9.5) 0/8 → 8/8, gated on a sealed independent
  holdout (`HoldoutVault`, EV-14, §9.6, ~50pp) and **never auto-promoted** — it stops
  at the release gate for human approval. `entelechy ops --out` persists the
  outcome in `ops-report.json` and the release decision in the hash-chained
  `audit-log.json`, re-verified by `entelechy audit`.

## Recommended next item

The measure → operate → re-study → release cycle is closed, and the first
operational drill (DR-1 backup/restore) is mechanized. Natural follow-ons:
1. **More operational drills (RB/AU/SL, P4).** DR-1 is now done; the remaining
   drills are rollback (RB), audit/attestation (AU) and SLO (SL). Mechanizing the
   PV-2 / DP-1 gaps (study-on-a-real-domain evidence; a formal deployment-profile
   approval step) would retire the last two TR-1 waivers — see the gaps table.
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
| Operational drills (RB/AU/SL) | P4 | DR-1 backup/restore **done** (`entelechy-artifacts::backup` + `entelechy drill`, DR-1 waiver retired); rollback/audit/SLO drills not yet mechanized |
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
counterfactual, diff, assure, release, serve, committee, audit, demo, ops, drill,
requirements [--check]` (+ `infer` under `--features openai`).
