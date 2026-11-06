# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, **only when the user asks** (handoff docs
are the exception — see CLAUDE.md).

> **WP1 (runtime-correctness gate), WP2 (runtime shadow write-interception) and the
> WP2 shadow→governance bridge are all COMPLETE, green, committed and pushed.**
> `HEAD = origin/main = 021b8f8`, working tree clean. The full acceptance gate passed
> this session at that commit: **0 test failures**, clippy/doc clean, `requirements
> --check` PASS at P5, every `drill --kind all` drill SATISFIED (incl. the new
> `shadow-governance`).

## START HERE — the tree is clean; pick the next unit with the user

No uncommitted work and no decision outstanding. The next unit is a choice (see
candidates below) — confirm it with the user before starting; each has a real
trade-off or is larger than a single quick unit.

Re-confirm green at the start of a session if desired (`-j 2`, resource policy
MEDIUM, **verify exit codes unpiped** — a `| tail` pipe masks cargo's real exit; see
How to work). Last full run (2026-11-06, at `021b8f8`) was fully clean:
- `cargo fmt --all --check` — clean
- `cargo clippy --workspace --all-targets -j 2 -- -D warnings` (+ gateway
  `-p entelechy-gateway --features wasm,openai-tls`) — clean
- `cargo test --workspace -j 2` — **0 failed** (workspace ≈398 tests incl. 3 new
  `shadow_governance` tests; a targeted `-p entelechy-cli` run adds ~32 more)
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` — clean
- `cargo run -p entelechy-cli -- requirements --check` — **PASS at P5**
- `cargo run -p entelechy-cli -- drill --kind all` — every drill **SATISFIED**, incl.
  `shadow` (runtime enforcement) and `shadow-governance` (the governance bridge)
- **No new deps**; **golden journal byte-stable at v2** (no bump).

## Candidate next units (nothing started — pick with the user)

- **WP1.5 / identity:** swap the FNV MAC placeholder for real Ed25519 signing behind the
  existing keyring interface (`entelechy-identity/src/keyring.rs`). **Decision for the
  user:** adds a crypto dependency — behind a feature (keeps the default
  dependency-light, placeholder stays default) or in the default build
  (security-by-default). That trade-off is why it was not auto-picked.
- **WP2 follow-on — true `Par` concurrency** (`exec.rs`, "sequential in this slice"
  today). Scoped, self-contained, no dep.
- **WP2 follow-on — full gateway contract** (usage/token-limits/seed/tool-calls on
  `ModelRequest`). NB: `ModelRequest` is **frozen at the 1.0 contract** (`model.rs`), so
  this triggers the frozen-contract bump procedure — heavier, not a quick unit. It is
  also what WS5 needs to tell a cleanly-rejected tool call from a lost acknowledgement.
- **P5 research target:** CAPES (Constraint-Aware Pareto Evolutionary Search) + the eval
  protocol/paper skeleton from the D1 dossier (see below). Possible later: a 3-level
  integrity lattice (Untrusted ≤ Validated ≤ Trusted) richer than today's binary `Taint`.

## Open decisions for the user (ask, don't guess)

- **Ed25519 dependency** — behind a feature, or in the default build? (see WP1.5 above).
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

- **WP2 shadow→governance bridge** — commit `021b8f8` (this session, 2026-11-06). New
  `entelechy-cli::shadow_governance` drives candidates through `Engine::shadow`, asserts
  `committed_no_effects()`, and feeds the *measured* substitution evidence into the
  unchanged `entelechy_release::run_shadow` — so the governance shadow stage decides on
  runtime-enforced substitution, not trusted `dry_run_supported` metadata. The comparison
  token is the substitution-invariant `(capability, request_hash)` effect signature.
  Exposed as `entelechy drill --kind shadow-governance` (and under `--kind all`); five
  proven invariants; 3 tests. `entelechy-release` stays a governance leaf (wiring in the
  CLI, as the ops loop does) — no new crate edge. No deps, no journal bump.
- **WP2 runtime shadow write-interception** — commit `6bfe132`; handoff refresh
  `7c8dcc5`. `Engine::shadow` + `ShadowResult`/`SubstitutedEffect`/
  `Journal::tool_effect_count`; red-team `tests/wp2_shadow.rs` + `drill --kind shadow`.
- **WP1 remainder — WS2 authority + WS3 parity + WS5 write-ahead + enforcement drill**
  — commit `3316c2f`; handoff refresh `222dac4`. Completes the WP1 gate.
- **WP1 WS1 + WS4 + shadow/canary rollout + WP1/SOTA docs** — commit `d0703df`.
- **PC-3 / phase gate → P5** — `e2ada81`. **`real_study` convergence** — `2c92106`.

The WP1 plan (`docs/wp1-correctness-gate.md`) is updated to reflect all of the above as
committed, and that shadow is now enforcement (not policy).

## How to work here (see CLAUDE.md for durable invariants)

- Build/test with `-j 2` — a cold `--workspace` build OOM-kills (exit 137).
- **Exit codes:** piping a cargo command to `| tail`/`| head` makes the shell report the
  *pipe's* exit code, not cargo's — a failed build can look like exit 0. Run gate
  commands **unpiped** (prefer a background run, whose completion notice carries the real
  exit code), or check `${PIPESTATUS[0]}`.
- Resource policy MEDIUM: ≤2 heavy local jobs; targeted validation first; full suite
  once at the end; heed CPU/RAM pressure notices. **This session the machine's CPU was
  repeatedly pegged at ~90–100% by external (non-Claude) load, in bursts lasting a few
  minutes with brief dips to near 0%.** A background `until cpu<70` poll (git-bash loop
  calling PowerShell `Get-CimInstance Win32_Processor` LoadPercentage) waited out each
  burst before launching the gate — that pattern worked well. RAM (the real OOM risk)
  stayed comfortable (≤80%) throughout. A single instantaneous CPU sample is noisy;
  average several, or just wait for a dip.
- `gh` is NOT authenticated: push over SSH (`git@github.com:mi7plus/entelechy.git`);
  GitHub reads via unauthenticated `curl https://api.github.com/repos/mi7plus/entelechy/...`.
- Windows + git-bash; **`git status` + 0-byte inspect before every commit** (stray
  0-byte files recur from shell quoting). Trust `git status --porcelain`.
- **Reading PDFs:** `Read` can't render PDFs here; Python 3.14 has `pymupdf`/`fitz` and
  `pdftotext` is on PATH. Use `python -` (NOT `-I`, which drops user site-packages);
  force UTF-8 stdout. The scratchpad is write-blocked by a guard — write scratch inside
  the repo or pipe via stdin.
- The journal is a **frozen contract** (`entelechy.journal`, **v2**). A wire-format
  change bumps `JOURNAL_FORMAT_VERSION`, updates `golden_journal.rs` bytes,
  `FROZEN_CONTRACTS`, `docs/contracts-1.0.md` and CHANGELOG — all together. WS5, WP2
  shadow and the governance bridge were all additive and did **not** bump it.
- **Governance/ops crates are leaves on purpose.** `entelechy-release` and
  `entelechy-ops` must not depend on `entelechy-runtime`/`entelechy-gateway`; runtime
  wiring lives in the CLI (the shadow→governance bridge and the online loop both follow
  this). Keep it that way unless the user decides otherwise.
- Conventions: each crate's `//!` cites its PRD section; commit messages reference
  requirement IDs / PRD sections; heavy engines behind features (`wasm`, `openai`,
  `openai-tls`).
- TR-1 phase gate: `entelechy requirements --check`; `CURRENT_GATE_PHASE` = P5 (final).
- Drills: `entelechy drill --kind restore|rollback|audit|slo|drift|degraded|rollout|enforcement|shadow|shadow-governance|all`.

## Background work at handoff

None running. The full gate completed this session (green, recorded above); the CPU
poll jobs all exited. No dev server, watcher, or unfinished build. The three commits of
this session (`6bfe132`, `7c8dcc5`, `021b8f8`) are all pushed to `origin/main`.
