# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, **only when the user asks** (handoff docs
are the exception — see CLAUDE.md).

> The through-line is **runtime-correctness hardening (WP1)** — making Entelechy's
> differentiator ("architecture discovery under *verifiable* runtime constraints")
> actually true. See [`docs/wp1-correctness-gate.md`](docs/wp1-correctness-gate.md).
> Order: WS1 ✅ → WS4 ✅ → **WS3 (next)** → WS2 → WS5.

## START HERE — validate, then do WS3

1. **Re-run the full gate to confirm green** (it was green at the end of last
   session: **374 passed, 0 failed**). `-j 2`, within the resource policy (MEDIUM;
   the machine was under heavy external CPU pressure last session — watch the
   Control Room notices and hold heavy jobs while over-ceiling):
   - `cargo fmt --all --check`
   - `cargo clippy --workspace --all-targets -j 2 -- -D warnings` (+ gateway
     `-p entelechy-gateway --features wasm,openai-tls`)
   - `cargo test --workspace -j 2`
   - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`
2. **Then build WS3** (next workstream of WP1): **policy + budget parity across
   execution modes** — the counterfactual `diverged` branch calls `tools.call`/
   `model.infer` with no `authorize_*` and no budget check that `Execute` runs
   (`exec.rs` Llm ~line 381, Tool ~line 481 — the `Mode::Counterfactual { diverged:
   true }` arms). Extract the `Execute` pre-dispatch guard into `guard_tool`/
   `guard_model` helpers and call them from **both** the `Execute` arm and the
   diverged counterfactual branch; thread a `&mut Counters` into
   `Mode::Counterfactual` (it has none today) so budgets are shared across the pre-
   and post-divergence spans. Red tests first (see the plan doc §WS3). Then WS2, WS5.

## Uncommitted working tree (nothing is committed — all gated on the user)

Three independent bodies of work sit green/uncommitted. The full gate is green
across all of them together (374 tests). `git status`:

- **WS4 of WP1 (NEW this session, verified):** deterministic replay from the
  *recorded* run input (RK-2). `crates/entelechy-runtime/src/journal.rs` (new
  `Journal::input: Value` field, `#[serde(default)]`; `JOURNAL_FORMAT_VERSION 1→2`;
  new `ReplayMismatch::InputMismatch`), `exec.rs` (`outcome_output` seeds the
  recorded input; `initial_input()` removed), `lib.rs` (`execute` records the input;
  `replay` verifies a supplied input and reports `InputMismatch` on a tamper;
  `counterfactual` dropped its input param and seeds from the journal + 3 new tests),
  `crates/entelechy-ir/src/value.rs` (`impl Default for Value` = `{}`),
  `crates/entelechy-cli/src/main.rs` (replay from `journal.input`; `counterfactual`
  drops the redundant `--input` flag). **Frozen-contract ripple (journal 1→2):**
  `crates/entelechy-contracts/src/freeze.rs`,
  `crates/entelechy-runtime/tests/golden_journal.rs` (bumped version assert + new
  canonical bytes — they matched on first run), `docs/contracts-1.0.md`, `CHANGELOG.md`.
  Verified: runtime **31/31** unit + **2/2** golden; full gate clean. A v1 journal
  still deserializes (input → `{}`). No new deps; phase gate unchanged.
- **WS1 of WP1 (prior session, verified):** `crates/entelechy-runtime/src/exec.rs`
  (Par/Map fold `ValueMeta::join` over input+children — no label laundering) and the
  4 live-path adversarial tests in `lib.rs`. These are the plan's SEC-01/SEC-02 tests.
- **Shadow/canary rollout unit (prior session, still uncommitted):**
  `crates/entelechy-release/src/rollout.rs` (new) + `lib.rs`, `Cargo.toml`,
  `crates/entelechy-cli/src/{drill.rs,main.rs}`, `tests/cli.rs`, `ARCHITECTURE.md`,
  `CHANGELOG.md`, `Cargo.lock`. **Caveat (from WP1):** shadow is a *governance* layer
  trusting effect metadata, not an executor write-interception — keep any "safety"
  wording as "policy" until WP1 (WS2–WS3) lands. Still fine to commit.
- **Docs (prior session):** [`docs/wp1-correctness-gate.md`](docs/wp1-correctness-gate.md)
  (WS1 + WS4 now marked done) and [`docs/state-of-the-art/`](docs/state-of-the-art/)
  (11 SOTA PDFs git-ignored, the annotated `README.md` tracked — `git add
  docs/state-of-the-art/` stages only `.gitignore` + `README.md`).

## WP1 — the runtime-correctness gate (the active workstream)

| WS | Gap | Status |
|----|-----|--------|
| WS1 | `Par`/`Map` launder IFC labels | ✅ **done + verified** |
| WS4 | replay reconstructs from `{}` (journal didn't persist the input) | ✅ **done + verified** (this session) |
| WS3 | counterfactual skips policy + budget (the `diverged` branch) | **next** — medium refactor |
| WS2 | `Delegate` doesn't narrow authority at runtime (`exec.rs` Delegate arm; `authorize_*` never take an envelope — the runtime ignores `program.authority`) | largest |
| WS5 | tool journaled *after* the effect (reuse `CommitStatus::Unknown`→`ReconciliationRequired` scaffolding) | boundary |

Acceptance for "WP1 green": the SEC/EFF/REP red-team suite, ideally an `entelechy
drill --kind enforcement`, full gate clean, **no new deps**, phase gate unchanged.
Full detail, red tests and fixes: the plan doc. **WS4/WS5 note:** both touch the
journal — WS4 already bumped the format to v2 and added `#[serde(default)]`-guarded
back-compat, so WS5's `Unknown`-intent change is an additive v2 change (no second
bump unless the entry/`kind` shape changes).

## External analyses to carry forward (context, not code)

Two independent strategy/research documents the user supplied; both **converge** with
WP1 and independently reproduce the same bug diagnosis. Neither is in the repo:

1. A Romanian **R&D/funding plan** (`~/Downloads`, pasted earlier) — TRL framing,
   PoCIDIF/Horizon/EIC funding, budgets. Honest; flags its own caveats.
2. **`~/Downloads/entelechy_research_dossier.html`** — a ChatGPT "D1.1–D1.5" dossier:
   formal semantics, an eval protocol (benchmarks B1–B4, baselines BL1–BL6, gates
   G0–G5), and **CAPES** (Constraint-Aware Pareto Evolutionary Search).

Assessment: **adopt** its G0/P1 (= WP1) and SEC/EFF/REP acceptance tests; **adopt as
P5 target** CAPES and the eval protocol/paper skeleton; **resist** (a) restructuring
the 22 real crates to its invented module names, and (b) its heavy infra
(PostgreSQL/S3/async-tokio/K8s) — it collides with the dependency-light invariant;
meet durability (WS5) with the existing in-process journal + WAL first. One idea for
later (WP2): a 3-level integrity lattice (Untrusted ≤ Validated ≤ Trusted) richer
than today's binary `Taint`.

## Open decisions for the user (ask, don't guess)

- **Commit the WS4 fix + WS1 fix + the new docs** (WP1 plan, SOTA folder)? — recommend
  yes; the full gate is green (374) and the journal 1→2 bump is self-contained and
  back-compatible. WS4's commit message should cite **RK-2** and the journal-format
  bump; WS1's cites IR-I9/PRD 7.5. (Consider one commit for WS1+WS4 docs or two —
  they are independent units.)
- **Commit the shadow/canary rollout unit**? (standing question; still gated.)
- **Research the Romanian grant specifics** (PoCIDIF deadlines/eligibility) in the
  browser? (The user flagged these need MySMIS/MIPE confirmation.)
- A **one-page scientific framing** (the C2+C3 reframing) — offered, not yet done.

## Prior units (committed + pushed)

- **PC-3 / phase gate → P5** — committed `e2ada81`, handoff `4c3bcae`. The requirements
  gate is maxed; no further gate advancement exists.
- **`real_study` convergence** — committed `2c92106`, handoff `9c89eec`.

## How to work here (see CLAUDE.md for durable invariants)

- Build/test with `-j 2` — a cold `--workspace` build OOM-kills (exit 137).
- Resource policy is MEDIUM: ≤2 heavy local jobs; prefer targeted validation; run the
  full suite once at the end; watch for mid-task CPU/RAM pressure notices (they were
  frequent and CRITICAL last session — `fmt`/`clippy --check` don't compile and are
  safe to run under pressure; defer `test`/`doc`/full builds while over-ceiling).
- `gh` is NOT authenticated: push over the SSH remote
  (`git@github.com:mi7plus/entelechy.git`); for GitHub reads use unauthenticated
  `curl https://api.github.com/repos/mi7plus/entelechy/...`.
- Windows + git-bash; `git status` and inspect before every commit (stray 0-byte files
  recur from shell quoting — last `git status` was clean).
- **Reading PDFs:** the `Read` tool can't render PDFs here (poppler/pdftoppm missing),
  but Python 3.14 has `pymupdf`/`fitz` and `pdftotext` is on PATH. Extract text via
  `python -` (NOT `-I`, which drops the user site-packages where fitz lives); force
  UTF-8 stdout (`sys.stdout.reconfigure(encoding="utf-8")`). The scratchpad is
  write-blocked by a guard — write scratch inside the repo or pipe via stdin.
- Conventions: each crate's `//!` cites its PRD section; commit messages reference
  requirement IDs / PRD sections; heavy engines stay behind features (`wasm`,
  `openai`, `openai-tls`).
- The journal is a **frozen contract** (`entelechy.journal`, now v2). A wire-format
  change bumps `JOURNAL_FORMAT_VERSION`, updates `golden_journal.rs` bytes,
  `FROZEN_CONTRACTS`, `docs/contracts-1.0.md` and CHANGELOG — all together (the
  golden test header spells out the procedure).
- TR-1 phase gate is `entelechy requirements --check`; `CURRENT_GATE_PHASE` = P5 (final).
- Operational drills: `entelechy drill --kind restore|rollback|audit|slo|drift|degraded|rollout|all`.
