# Next session — Entelechy

_Handoff finalized 2026-11-06 (Control Room run #10, session 2: the two
previously-pending units were committed and pushed to `main`, and both were
re-verified green this session)._

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, **only when the user asks** (handoff docs
are the exception — see CLAUDE.md).

> **The two pending units are now COMMITTED and PUSHED.** `main` advanced
> `07e0a57 → ea9e06e → e810655`, and this handoff-refresh commit sits directly atop
> `e810655`. `HEAD = origin/main`. The working tree is otherwise **clean — no units
> are pending.** The long-standing "commit the pending units?" question is resolved
> (done); Ed25519 was kept **opt-in** (dependency-light default preserved).

## What landed this session (run #10, session 2)

Two self-contained units, committed and pushed (nothing else touched):

1. **Codegen conformance backend — `ea9e06e`** (PRD §21 Phase 4, Q10). The compiled
   backend (`entelechy-runtime::codegen`) now lowers + enforces the full
   deterministic control-flow core (nested `Seq`, `Branch`, bounded `Loop`, `Map`,
   `Retrieval`), the model-call budget + egress authorization with the interpreter's
   real failure node paths, and `Gate`/`Tool`/`Delegate` with WS2/WS3 parity. Only
   `Par` and `Human` remain `Unsupported` (deferred by design, not omission — see
   below). `CompiledPlan`/`CompiledStep` is **not** a frozen contract. 5 files,
   +1404/−170.
2. **Ed25519 approval signatures — `e810655`** (PRD 5.9 signature agility, AU-2).
   Real RFC 8032 Ed25519 behind the unchanged `KeyRing` interface, gated by a new
   **opt-in** `ed25519` feature; the default build keeps the FNV-1a placeholder and
   pulls in no crypto dep (dependency-light default intact). 6 files, +340/−14.

Both commits are authored George Olteanu with the `Co-Authored-By: Claude Opus 4.8`
trailer. The four `CHANGELOG.md ### Added` entries were split cleanly across the two
commits (87 + 21 = 108 insertions — the whole original block, lossless).

## START HERE

1. `main` is up to date and the tree is clean (only this handoff was uncommitted at
   finalization). **Nothing to commit from last session.**
2. **The one open decision is which unit to build next — every candidate needs a
   decision (see "Open decision"). Ask the user; don't guess.** The decision-free
   codegen thread is finished, and the TR-1 gate already attests every normative
   requirement through P5 (the final roadmap phase) with no waivers, so the core
   roadmap is effectively complete.
3. `git status` + 0-byte inspect before every commit (CLAUDE.md). Push over SSH
   (`git@github.com:mi7plus/entelechy.git`) only when asked. `gh` is NOT
   authenticated; GitHub reads via unauthenticated
   `curl https://api.github.com/repos/mi7plus/entelechy/...`.
4. Build/verify under the resource rules: `-j 2` (a cold `--workspace` build
   OOM-kills, exit 137); poll CPU/RAM before any `--workspace` run — this machine's
   CPU sits bursty right around the 70% ceiling (it crossed it repeatedly this
   session). Run gate commands unpiped and read `${PIPESTATUS[0]}`; foreground
   `sleep` is blocked (use a background `until`-loop or Monitor). Control Room emits
   live pressure notices — adapt promptly.

## Verification status at this handoff

Re-run **this session** against the committed code:
- `cargo fmt --all --check` — clean.
- `cargo test -p entelechy-runtime -j 2` — **exit 0** (lib incl. codegen conformance,
  + golden 2, wp1 8, wp2 5).
- `cargo test -p entelechy-identity --features ed25519 -j 2` — **exit 0** (18 tests,
  incl. the 4 crypto-path tests).

The **full** workspace gate (clippy `-D warnings` incl. the `wasm,openai-tls` and
`ed25519` feature paths, `cargo test --workspace`, rustdoc `-D warnings`,
`requirements --check` PASS at P5, every drill SATISFIED) was green at the previous
handoff against **byte-identical** code and was **not** re-run in full this session
(the code did not change — only the changelog split, which returned to the identical
bytes — and CPU was repeatedly over the 70% ceiling). A full-gate re-run is only
needed once you edit code. If you want belt-and-suspenders before the next unit, the
full gate is in CLAUDE.md.

## Open decision (ask the user — don't guess)

**Which next unit?** All need a decision; recommended pick first:

| Candidate | What it buys | Why it needs a decision |
|---|---|---|
| **Full gateway contract** *(rec)* | usage/token-limits/seed/tool-calls on `ModelRequest`; lets the runtime distinguish a cleanly-rejected `Tool` from a lost ack (today both fold into `ReconciliationRequired`) | `ModelRequest` is **frozen at the 1.0 contract** → triggers the frozen-contract bump procedure |
| True `Par` concurrency (`exec.rs`) | real parallelism in the interpreter | collides with the byte-stable journal (RK-2) unless tokio (breaks dependency-light) or deterministic journal-segment splicing is designed |
| CAPES research + eval protocol | P5 research target (Constraint-Aware Pareto Evolutionary Search) + paper skeleton from the D1 dossier | larger, research-flavoured |
| Pause new units | — | roadmap gate is complete at P5; remaining items are genuinely optional/forward-looking |

Also still open if the user wants them (offered in earlier sessions, not started):
- **Romanian grant specifics** (PoCIDIF deadlines/eligibility) — needs MySMIS/MIPE
  confirmation in the browser.
- A **one-page scientific framing** (the C2+C3 reframing).

## Why `Par`/`Human` are deferred in codegen (not an omission)

Lowering `Par` would pin a *sequential* semantics a future truly-concurrent
interpreter could diverge from; `Human` needs an interactive responder (Phase 2+).
Don't lower either without the matching decision. The interpreter stays the
conformance **reference** executor (Q10); codegen is a candidate gated by
`CodegenEvidence::codegen_eligible`.

## Durable invariants (full list in CLAUDE.md)

- Every crate `#![forbid(unsafe_code)]`; `missing_docs = "warn"`; rustdoc `-D
  warnings` enforces documented public items. Each crate's `//!` cites its PRD
  section(s); commit messages reference PRD requirement IDs.
- Keep the default build dependency-light; heavy deps go behind features
  (**`ed25519`** in `entelechy-identity`, `wasm`/`openai`/`openai-tls` in the
  gateway). Clippy the feature paths (lines in CLAUDE.md).
- The journal is a **frozen contract** (`entelechy.journal`, **v2**); untouched by
  this session. `CompiledPlan`/`CompiledStep` and approval signatures are **not**
  frozen contracts. The `entelechy.native-request` (`ModelRequest`) shape **is**
  frozen — the "full gateway contract" unit above would bump it.
- The codegen backend reuses the interpreter's `eval_condition`/`get_path`/
  `merge_args` and `authorize_model`/`authorize_tool`, so a policy/authority denial
  carries the identical reason on both backends by construction.
- Governance/ops crates are leaves on purpose (`entelechy-release`/`-ops` must not
  depend on `-runtime`/`-gateway`; runtime wiring lives in the CLI).
- TR-1 phase gate: `entelechy requirements --check`; `CURRENT_GATE_PHASE` = P5.
- Drills: `entelechy drill --kind restore|rollback|audit|slo|drift|degraded|rollout|
  enforcement|shadow|shadow-governance|all`.

## Prior units (committed + pushed — context)

- **This session:** codegen conformance (`ea9e06e`), Ed25519 opt-in (`e810655`).
- **Earlier:** WP2 shadow→governance bridge (`021b8f8`) + handoff `07e0a57`; WP2
  runtime shadow write-interception (`6bfe132`); WP1 remainder (`3316c2f`). WP1/WP2,
  the governance bridge, the staged-rollout machinery, and the TR-1 advance to P5
  are all complete, green, committed and pushed.

## Background work at handoff

None running. No dev server, watcher, or unfinished build. Progress was tracked via
Control Room milestones (all 4 complete) and the in-terminal progress bar.
