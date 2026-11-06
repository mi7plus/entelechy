# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, **only when the user asks** (handoff docs
are the exception — see CLAUDE.md).

> **WP1 (runtime-correctness gate) and WP2 (runtime shadow write-interception) are
> both COMPLETE, green, committed and pushed.** WP1 = WS1 ✅ WS2 ✅ WS3 ✅ WS4 ✅
> WS5 ✅ + enforcement drill ✅ (`d0703df` + `3316c2f`). WP2 = `Engine::shadow`
> runtime effect-substitution + red-team suite + shadow drill (`6bfe132`). The full
> acceptance gate is green: **392 passed / 0 failed**, clippy clean, doc clean,
> `requirements --check` PASS at P5, `drill --kind all` every drill SATISFIED.

## START HERE — the tree is clean; pick the next unit with the user

No uncommitted work and no decision outstanding from WP2. The next unit is a choice
(see candidates below) — confirm it with the user before starting, since each has a
real trade-off or is a larger effort than a single quick unit.

Re-confirm green at the start of a session if desired (`-j 2`, resource policy
MEDIUM, and **verify exit codes unpiped** — a `| tail` pipe masks cargo's real exit;
see How to work). Last full run (2026-11-06) was fully clean:
- `cargo fmt --all --check` — clean
- `cargo clippy --workspace --all-targets -j 2 -- -D warnings` (+ gateway
  `-p entelechy-gateway --features wasm,openai-tls`) — clean
- `cargo test --workspace -j 2` — **392 passed, 0 failed**
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` — clean
- `cargo run -p entelechy-cli -- requirements --check` — **PASS at P5**
- `cargo run -p entelechy-cli -- drill --kind all` — every drill **SATISFIED**,
  incl. the **shadow write-interception** drill
- **No new deps** since WP1; **golden journal byte-stable at v2** (no bump).

## Candidate next units (nothing started — pick with the user)

- **WP1.5 / identity:** swap the FNV MAC placeholder for real Ed25519 signing behind the
  existing keyring interface (`entelechy-identity/src/keyring.rs`). **Decision for the
  user:** this adds a crypto dependency — put it behind a feature (keeps the default
  dependency-light but leaves the placeholder as default) or accept it in the default
  build (security-by-default). That trade-off is why it was not auto-picked.
- **WP2 follow-on — true `Par` concurrency** (`exec.rs`, "sequential in this slice"
  today). Scoped, self-contained.
- **WP2 follow-on — wire the runtime shadow into governance.** The new runtime
  `Engine::shadow` is the *enforcement* layer; the release-layer
  `entelechy_release::run_shadow` is still the *governance* layer (eligibility +
  outcome comparison) and was deliberately **not** rewired to consume runtime shadow
  journals. A reasonable optional follow-up: have `run_shadow` drive the candidate
  through `Engine::shadow` and assert `committed_no_effects()` instead of trusting
  `dry_run_supported` metadata. (Noted at the time of the WP2 commit; not required.)
- **Full gateway contract** (usage/token-limits/seed/tool-calls on `ModelRequest`).
  NB: `ModelRequest` is **frozen at the 1.0 contract** (`model.rs`), so this triggers the
  frozen-contract bump procedure — heavier, not a quick unit. It is also what WS5 needs to
  tell a cleanly-rejected tool call from a lost acknowledgement.
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

- **WP2 — runtime shadow write-interception** — commit `6bfe132` (this session,
  2026-11-06). `Engine::shadow` substitutes every consequential tool effect (gateway
  never called) while WS1/WS2/WS3 stay live; `ShadowResult`/`SubstitutedEffect`/
  `Journal::tool_effect_count`; red-team `tests/wp2_shadow.rs` + `drill --kind shadow`.
  No journal bump, no new deps, TR-1 gate unchanged. Full gate green; pushed.
- **WP1 remainder — WS2 authority + WS3 parity + WS5 write-ahead + enforcement drill**
  — commit `3316c2f`; handoff refresh `222dac4`. Completes the WP1 gate.
- **WP1 WS1 + WS4 + shadow/canary rollout + WP1/SOTA docs** — commit `d0703df`.
- **PC-3 / phase gate → P5** — committed `e2ada81`. Requirements gate is maxed.
- **`real_study` convergence** — committed `2c92106`.

## How to work here (see CLAUDE.md for durable invariants)

- Build/test with `-j 2` — a cold `--workspace` build OOM-kills (exit 137).
- **Exit codes:** piping a cargo command to `| tail`/`| head` makes the shell report the
  *pipe's* exit code, not cargo's — a failed build can look like exit 0. Run gate
  commands **unpiped** (and prefer a background run, whose completion notice carries the
  real exit code), or check `${PIPESTATUS[0]}`.
- Resource policy MEDIUM: ≤2 heavy local jobs; targeted validation first; run the full
  suite once at the end; heed mid-task CPU/RAM pressure notices (Control Room refuses
  extra heavy jobs while over a ceiling). This session the machine's CPU sat pegged at
  100% from external load for a while — a background `until cpu<70` poll waited it out
  before the gate ran; RAM (the real OOM risk) stayed comfortable throughout.
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
  `FROZEN_CONTRACTS`, `docs/contracts-1.0.md` and CHANGELOG — all together (the golden
  test header spells out the procedure). WS5 and WP2 shadow were both additive and did
  **not** bump it.
- Conventions: each crate's `//!` cites its PRD section; commit messages reference
  requirement IDs / PRD sections; heavy engines behind features (`wasm`, `openai`,
  `openai-tls`).
- TR-1 phase gate: `entelechy requirements --check`; `CURRENT_GATE_PHASE` = P5 (final).
- Operational drills: `entelechy drill --kind restore|rollback|audit|slo|drift|degraded|rollout|enforcement|shadow|all`.

## Background work at handoff

None running. The full gate (fmt/clippy/test/doc/requirements/drill) completed this
session — all green; results recorded above. No dev server, watcher, or unfinished build.
