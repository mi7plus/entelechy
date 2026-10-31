# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, **only when the user asks** (handoff docs
are the exception — see CLAUDE.md).

## DONE (this session) — advanced the TR-1 gate P1 → P4, COMMITTED & PUSHED

The whole "advance the phase gate from P1 to P4" unit is **complete, verified, and
committed to `main` as `2ac6002` (pushed to `origin`)**. The repo builds, the full
gate is green, and `requirements --check` now passes at **P4**. Nothing is pending
on this unit; pick up from "After the gate reaches P4" below.

### Verified state (run this session, all green)

HEAD `main` is `2ac6002` ("Advance TR-1 phase gate P1→P4 …"). At that commit:

- `cargo fmt --all --check` — clean
- `cargo clippy --workspace --all-targets -- -D warnings` — clean
- `cargo clippy -p entelechy-gateway --all-targets --features wasm,openai-tls -- -D warnings` — clean
- `cargo test --workspace -j 2` — **340 passed, 0 failed** (was 337; +3 DX-1 quickstart tests)
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` — clean
- `cargo run -p entelechy-cli -- requirements --check --phase p4` — **PASS**

Build with `-j 2` (a cold `--workspace` build OOM-kills, exit 137).

`git status` is clean of stray 0-byte files (4 were created and removed this session
from git-bash quoting — unquoted `()<>` in grep/find args; **inspect `git status`
before every commit**).

Modified (16): `CHANGELOG.md`, `Cargo.lock`, `NEXT_SESSION_PROMPT.md`,
`crates/entelechy-artifacts/src/{backup.rs,lib.rs,platform.rs}`,
`crates/entelechy-cli/src/{drill.rs,main.rs,release.rs}`,
`crates/entelechy-contracts/src/traceability.rs`,
`crates/entelechy-domain/src/lib.rs`, `crates/entelechy-integration/Cargo.toml`,
`crates/entelechy-ops/src/lib.rs`,
`crates/entelechy-release/src/{drill.rs,lib.rs,slo.rs}`.
New (4): `crates/entelechy-cli/src/quickstart.rs`,
`crates/entelechy-domain/src/promotion.rs`,
`crates/entelechy-ops/src/degraded.rs`,
`crates/entelechy-integration/tests/recovery.rs`.

### What the P4 gate required, and how it is met

The P4 gate needs records for 7 requirements beyond P1 (`requirements --check
--phase p4`): PV-3, EL-1, PC-2, DR-2, DR-3, OL-2, DX-1. All are backed by real
executed code with tests/drills and recorded in
`entelechy_contracts::phase_gate_evidence()`; **no waivers remain**.

| Req | Mechanism | Verification artifact |
|---|---|---|
| **DX-1** | `entelechy-cli::quickstart` — declared new-user path (install→build→`init`→`study --real`), budgets sum < 30 min; drives `real_study()` in-process, confirms a green report | `quickstart_reaches_first_report_within_30_minutes` (+2); `entelechy quickstart` |
| **PV-3** | `entelechy-domain::promotion::drift_drill` | `drift_drill_satisfies_pv3` (+3); `entelechy drill --kind drift` |
| **OL-2** | `entelechy-ops::degraded::degraded_mode_drill` | `degraded_mode_drill_satisfies_ol2` (+3); `entelechy drill --kind degraded` |
| **DR-2** | `entelechy-artifacts::backup` revocation + reconcile-first on `RestoreReport` | `revocation_survives_restore`, `post_restore_effects_are_reconciled_first`; `entelechy drill --kind restore` |
| **DR-3** | `platform::assess_evidence_state` + `release::promote` `evidence_state` gate | cross-crate `entelechy-integration/tests/recovery.rs` |
| **PC-2** | `entelechy-protocol::negotiate_secure` (pre-existing) | `no_silent_security_downgrade` |
| **EL-1** | `entelechy-eval::HoldoutVault` (pre-existing) | `design_plane_is_refused` + 2 |

CLI additions this unit: `entelechy drill --kind drift|degraded` (new printers), the
`restore` drill now also shows the DR-2 revocation-survival / reconcile-first
invariants (populated from a real revoked release id and an in-flight effect keyed
by a genuine `entelechy_effects::OperationKey`), and a new `entelechy quickstart
[--out]` command. `CURRENT_GATE_PHASE` is now `Phase::P4`; its doc comments and the
`phase_gate_evidence()` doc are updated.

### Commit — DONE

The user green-lit the commit, so the whole unit is committed as `2ac6002` and
pushed to `origin/main` (one commit referencing the 7 requirement IDs). This handoff
refresh is a follow-up doc-only commit (handoff docs are the CLAUDE.md commit
exception). Nothing else is pending on this unit.

## After the gate reaches P4

The gate is at the project's furthest enforced phase. Natural follow-ons (unchanged):

1. **Unify the two studies / drive repair through the real proposer loop** — the
   simulated `study` still scores by structure and the re-study repair is a
   hand-authored typed patch; fold the real executor in as the scoring function
   and/or point the re-study at the real `search`/`design` proposer.
2. **Exercise the level-2 retrieval path end to end** (add a retrieval-miss failure
   class so a run visibly unlocks retrieval).
3. **Shadow/canary rollout, web console, SDKs (P4/P3)** — not implemented.
4. **P5**: multi-tenancy, plugin registry (PC-3, the only unrecorded requirement —
   first enforced at P5), learned priors, distillation. Advancing the gate to P5
   means recording PC-3 and bumping `CURRENT_GATE_PHASE`.

## How to work here (see CLAUDE.md for durable invariants)

- Build/test with `-j 2` — a cold `--workspace` build OOM-kills (exit 137).
- `gh` is NOT authenticated: push over the SSH remote; for PR/CI status use
  unauthenticated `curl https://api.github.com/repos/mi7plus/entelechy/...`.
- Windows + git-bash; `git status` and inspect before every commit (stray 0-byte
  files recur from shell quoting — quote grep/command args containing `()><`).
- Conventions: each crate's `//!` cites its PRD section; commit messages reference
  requirement IDs; heavy engines stay behind features (`wasm`, `openai`,
  `openai-tls`).
- TR-1 phase gate is `entelechy requirements --check`; evidence lives in
  `entelechy_contracts::phase_gate_evidence()` with `CURRENT_GATE_PHASE`.

The CLI surface: `init, objective, capability, eval, design, study [--real] [--out],
trace, replay, counterfactual, diff, assure, release, serve, committee, audit, demo,
ops, drill [--kind restore|rollback|audit|slo|drift|degraded|all], quickstart [--out],
requirements [--check [--phase pN]]` (+ `infer` under `--features openai`).
