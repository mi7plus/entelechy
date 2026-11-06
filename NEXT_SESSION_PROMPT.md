# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, **only when the user asks** (handoff docs
are the exception — see CLAUDE.md).

> The through-line has been **runtime-correctness hardening (WP1)** — making
> Entelechy's differentiator ("architecture discovery under *verifiable* runtime
> constraints") actually true. Plan: [`docs/wp1-correctness-gate.md`](docs/wp1-correctness-gate.md).
> **WP1 is now COMPLETE and green** — all five workstreams plus the acceptance drill
> landed: WS1 ✅ → WS4 ✅ → WS3 ✅ → WS2 ✅ → WS5 ✅, drill ✅.

## START HERE — the WP1 unit is done and green, awaiting the user's commit decision

The working tree holds the **entire remaining WP1 unit** (WS2 + WS3 + WS5 + the
enforcement drill + docs), fully implemented, validated, and **uncommitted**. WS1 +
WS4 were already committed as `d0703df`.

1. **Decision needed (do not guess): commit the WP1 remainder?** Recommend **yes** —
   it is green, self-contained, and completes the WP1 gate. Suggested scope / PRD IDs
   for the message(s): WS2 (**A2 / IR-I6, PRD §5.4**), WS3 (**PRD §8.3, §10.2 / RK-8**),
   WS5 (**PRD §7.6, §8.5**), and the acceptance drill (**PRD §8, §5.4, §7.5, §8.5**).
   Can be one "WP1 complete" commit or split per workstream — the user's call. Push is
   over SSH (`gh` unauthenticated). **Run `git status` + the 0-byte check first.**
2. **Re-confirm green if desired** (`-j 2`, resource policy MEDIUM). Last run
   (2026-11-06) was fully clean:
   - `cargo fmt --all --check` — clean
   - `cargo clippy --workspace --all-targets -j 2 -- -D warnings` (+ gateway
     `-p entelechy-gateway --features wasm,openai-tls`) — clean
   - `cargo test --workspace -j 2` — **~386 passed, 0 failed** (runtime **35** unit +
     **2** golden + **8** `wp1_enforcement`)
   - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` — clean
   - `cargo run -p entelechy-cli -- requirements --check` — **PASS at P5**
   - `cargo run -p entelechy-cli -- drill --kind all` — every drill **SATISFIED**,
     incl. the WP1 WS1–WS5 enforcement drill
   - **No new deps** (no `Cargo.toml` change).

## Uncommitted working tree (all green, gated on the user)

`git status` — 6 modified + 2 new:

- `crates/entelechy-runtime/src/exec.rs`
  - **WS2:** `Delegate { authority, body }` arm — halts (`FailureReason::AuthorityDenied`)
    if the delegated envelope widens the active one; else pushes the narrowed envelope,
    runs the body, pops.
  - **WS5:** the `Execute` tool arm journals a write-ahead `ToolEffect { response: Null,
    commit: Unknown }` **before** `tools.call`, then resolves that entry in place to the
    real response + `CommitStatus::Committed` on `Ok`; an `Err` leaves the dangling
    `Unknown` and halts with `RunStatus::ReconciliationRequired`.
  - **WS3 (prior session):** free fns `guard_model`/`guard_tool` (authorize-then-budget),
    called from both the `Execute` arm and the diverged `Counterfactual` arm.
- `crates/entelechy-runtime/src/lib.rs`
  - **WS2:** `Engine.authority_stack: Vec<AuthorityEnvelope>`, seeded from
    `program.authority` in `execute`/`replay`/`counterfactual`; `authorize_tool` denies a
    capability outside the active envelope when inside a delegated scope (depth > 1),
    independent of policy. New `FailureReason::AuthorityDenied(String)`.
    `pub mod drill; pub use drill::{enforcement_drill, EnforcementDrillReport};`
  - **WS3 (prior session):** `Engine::counterfactual` seeds a fresh `Counters`; +3 CF tests.
  - Also holds WS1's inline adversarial IFC tests (from the prior session).
- `crates/entelechy-runtime/src/drill.rs` **(new)** — `enforcement_drill()` +
  `EnforcementDrillReport` (9 invariant bools + `ok`); probes `probe_ws1_ifc` …
  `probe_ws5_writeahead`; a `#[cfg(test)]` self-check.
- `crates/entelechy-runtime/tests/wp1_enforcement.rs` **(new)** — 8 public-API integration
  tests: WS2 (`delegate_narrows_active_authority`, `delegate_rejects_runtime_widening`,
  `nested_delegates_narrow_monotonically`, `data_dependent_capability_cannot_escape_delegate`,
  `delegate_free_programs_behave_identically`); WS5 (`tool_intent_is_journaled_before_dispatch`,
  `crash_between_dispatch_and_commit_is_recoverable`,
  `tool_intent_resolves_to_committed_on_acknowledgement`).
- `crates/entelechy-cli/src/drill.rs` — `DrillKind::Enforcement` + `run_enforcement`
  (grouped WS1–WS5 output), wired into `--kind all`.
- `CHANGELOG.md`, `docs/wp1-correctness-gate.md` — WP1 marked complete/green.

> [!NOTE]
> **WS5 did NOT bump the journal format.** A completed run still holds exactly one
> `ToolEffect` per tool node (the `Unknown` intent is resolved in place to `Committed`),
> so the golden journal is byte-stable at **v2** (WS4's bump). Distinguishing a cleanly
> rejected call from a lost acknowledgement needs the richer gateway contract — deferred
> to WP2; both currently surface as `ReconciliationRequired` (fail-safe).

> [!WARNING]
> **Stray 0-byte files recur** from git-bash quoting quirks. This session three appeared
> (`1`, `AuthorityEnvelope`, `ComparisonReport`) and were removed before any further work;
> the prior session hit the same. **Run `git status` + inspect for 0-byte files before
> every commit** (it is in CLAUDE.md).

## WP1 — the runtime-correctness gate (COMPLETE)

| WS | Gap | Status |
|----|-----|--------|
| WS1 | `Par`/`Map` launder IFC labels | ✅ committed `d0703df` |
| WS4 | replay reconstructed from `{}` (journal didn't persist input) | ✅ committed `d0703df` |
| WS3 | counterfactual skipped policy + budget (the `diverged` branch) | ✅ green, uncommitted |
| WS2 | `Delegate` didn't narrow authority at runtime; runtime ignored `program.authority` | ✅ green, uncommitted |
| WS5 | tool journaled *after* the effect | ✅ green, uncommitted |
| gate | `drill --kind enforcement` + full red-team suite + full gate | ✅ green, uncommitted |

"WP1 green" acceptance (all met): the SEC/EFF/REP red-team suite passes incl. every
adversarial case; `entelechy drill --kind enforcement` reports all invariants SATISFIED;
full gate clean; **no new deps**; TR-1 phase gate unchanged (PASS at P5). Full detail:
the plan doc.

## After WP1 — candidate next units (nothing started)

WP1 hardened the core; these are the plan doc's **"Explicitly out of WP1 (with homes)"**
items plus the P5-target research track. Pick with the user:

- **WP1.5 / identity:** swap the FNV MAC placeholder for real Ed25519 signing behind the
  existing keyring interface (`entelechy-identity/src/keyring.rs`).
- **WP2 — shadow write-interception** (`entelechy-release/src/rollout.rs`): make shadow a
  real enforcement guarantee (today it is a governance layer trusting `dry_run_supported`
  metadata; its safety now rests on the WP1 WS1/WS2 machinery).
- **WP2 — true `Par` concurrency** (`exec.rs`, "sequential in this slice" today).
- **WP2 — full gateway contract** (usage/token-limits/seed/tool-calls on `ModelRequest`);
  this is also what WS5 needs to tell a rejected call from a lost acknowledgement.
- **P5 research target:** CAPES (Constraint-Aware Pareto Evolutionary Search) + the eval
  protocol/paper skeleton from the D1 dossier (see below). Possible later: a 3-level
  integrity lattice (Untrusted ≤ Validated ≤ Trusted) richer than today's binary `Taint`.

## Open decisions for the user (ask, don't guess)

- **Commit the WP1 remainder?** — recommend yes (see START HERE for scope + PRD IDs).
- **Research the Romanian grant specifics** (PoCIDIF deadlines/eligibility) in the
  browser? (The user flagged these need MySMIS/MIPE confirmation.)
- A **one-page scientific framing** (the C2+C3 reframing) — offered, not yet done.

## External analyses to carry forward (context, not code)

Two independent strategy/research documents the user supplied; both **converge** with
WP1 and independently reproduce the same bug diagnosis. Neither is in the repo:

1. A Romanian **R&D/funding plan** (`~/Downloads`, pasted earlier) — TRL framing,
   PoCIDIF/Horizon/EIC funding, budgets. Honest; flags its own caveats.
2. **`~/Downloads/entelechy_research_dossier.html`** — a ChatGPT "D1.1–D1.5" dossier:
   formal semantics, an eval protocol (benchmarks B1–B4, baselines BL1–BL6, gates
   G0–G5), and **CAPES** (Constraint-Aware Pareto Evolutionary Search).

Assessment: its G0/P1 (= WP1) and SEC/EFF/REP acceptance tests are **done**; **adopt as
P5 target** CAPES and the eval protocol/paper skeleton; **resist** restructuring the
22 crates to its invented module names, and its heavy infra (PostgreSQL/S3/tokio/K8s) —
it collides with dependency-light; durability (WS5) is met with the in-process journal +
write-ahead intent first.

## Prior units (committed + pushed)

- **WP1 WS1 + WS4 + shadow/canary rollout + WP1/SOTA docs** — commit `d0703df`
  (WS1's IFC fix, WS4's recorded-input replay + journal v2, and the Phase-4 shadow/canary
  rollout machinery landed atomically — diffs interleaved across
  `exec.rs`/`lib.rs`/`main.rs`/`CHANGELOG`).
- **PC-3 / phase gate → P5** — committed `e2ada81`. Requirements gate is maxed.
- **`real_study` convergence** — committed `2c92106`.

## How to work here (see CLAUDE.md for durable invariants)

- Build/test with `-j 2` — a cold `--workspace` build OOM-kills (exit 137).
- Resource policy MEDIUM: ≤2 heavy local jobs; targeted validation first; run the full
  suite once at the end; heed mid-task CPU/RAM pressure notices.
- `gh` is NOT authenticated: push over SSH (`git@github.com:mi7plus/entelechy.git`);
  GitHub reads via unauthenticated `curl https://api.github.com/repos/mi7plus/entelechy/...`.
- Windows + git-bash; **`git status` + 0-byte inspect before every commit** (stray
  0-byte files recur from shell quoting — happened this session, caught pre-commit).
- **Reading PDFs:** `Read` can't render PDFs here; Python 3.14 has `pymupdf`/`fitz` and
  `pdftotext` is on PATH. Use `python -` (NOT `-I`, which drops user site-packages);
  force UTF-8 stdout. The scratchpad is write-blocked by a guard — write scratch inside
  the repo or pipe via stdin.
- The journal is a **frozen contract** (`entelechy.journal`, now **v2**). A wire-format
  change bumps `JOURNAL_FORMAT_VERSION`, updates `golden_journal.rs` bytes,
  `FROZEN_CONTRACTS`, `docs/contracts-1.0.md` and CHANGELOG — all together (the golden
  test header spells out the procedure). WS5 was additive and did **not** bump it.
- Conventions: each crate's `//!` cites its PRD section; commit messages reference
  requirement IDs / PRD sections; heavy engines behind features (`wasm`, `openai`,
  `openai-tls`).
- TR-1 phase gate: `entelechy requirements --check`; `CURRENT_GATE_PHASE` = P5 (final).
- Operational drills: `entelechy drill --kind restore|rollback|audit|slo|drift|degraded|rollout|enforcement|all`.
