# Next session — Entelechy

_Handoff finalized 2026-11-07 (Control Room run #10, session 3). The tool-effect
failure taxonomy — the unit session 2 left fully specced — is now **implemented,
gated green and committed** as `afa1717` (local only, **not pushed**). Beyond it, the
roadmap/TR-1 gate is complete at P5 with no waivers, so every remaining item is a
user-priority call (see "Open decisions")._

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push to
`main` after each self-contained unit, **only when the user asks** (handoff docs are
the exception — see CLAUDE.md).

> **`HEAD = afa1717` (taxonomy unit), one commit ahead of `origin/main = c170a66`.
> Working tree is CLEAN — nothing uncommitted. `afa1717` is NOT pushed** (push stays
> gated on an explicit user ask). The full gate was re-run green against it this
> session (see State).

## State

Shipped and pushed earlier this run (all green):
- `ea9e06e` — codegen conformance backend (control-flow + budget/egress + Gate/Tool/
  Delegate lowering). PRD §21 Phase 4, Q10.
- `e810655` — Ed25519 approval signatures behind the opt-in `ed25519` feature. PRD 5.9,
  AU-2. Kept **opt-in** (dependency-light default preserved).
- `868462a` / `c170a66` — handoff-doc refreshes.

Committed this session (session 3), **local only — NOT pushed**:
- `afa1717` — **tool-effect failure taxonomy** (PRD 8.5/8.6, RS-1; Q10 parity). A failed
  consequential tool call now splits on `ToolError::effect_outcome()`: a definitive
  rejection (`UnknownCapability`/`Rejected` → `NoEffect`) truncates the speculative
  write-ahead intent and halts a clean `Failed { ToolError }`; a lost/indeterminate
  acknowledgement (plain `Failed` → `Indeterminate`) still halts `ReconciliationRequired`.
  `register_guarded` marks a tool that validates and refuses before any side effect.
  Codegen mirrors the split for Q10 parity. Non-frozen; no new dep; journal wire format
  unchanged. 8 files, +320/-27.

Verification (session 3, against `afa1717` — the **full gate, re-run green**, `-j 2`,
under sustained ~100% CPU / ~87% RAM external load; targeted builds did not OOM):
- `cargo fmt --all --check` — clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — exit 0.
- `cargo clippy -p entelechy-gateway --all-targets --features wasm,openai-tls -- -D warnings` — exit 0.
- `cargo clippy -p entelechy-identity --all-targets --features ed25519 -- -D warnings` — exit 0.
- `cargo test --workspace -j 2` — exit 0 (55 test-result groups, 0 failures; incl. the 3
  new gateway tests, codegen `tool_rejection_conforms`, and the 2 red-team clean-fail
  probes; the 3 WS5 reconciliation probes still green as regression guards).
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` — exit 0 (new intra-doc
  links resolve).
- `entelechy requirements --check` — PASS @ P5. `entelechy drill --kind all` — every
  drill SATISFIED (WS5 drill still shows unacknowledged→reconcile, acknowledged→commit).

## START HERE — the specced unit is DONE; next is a user-priority call

The tool-effect failure taxonomy (session 2's spec) is **implemented, gated and
committed** as `afa1717`. **Two things await the user:**

1. **Push `afa1717` to `origin/main`?** Push stays gated on an explicit user ask
   (CLAUDE.md). Push over SSH: `git push origin main`.
2. **What next?** The roadmap/TR-1 gate is complete at P5 with **no waivers**, so beyond
   the taxonomy everything is optional/forward-looking — pick from "Open decisions"
   below (or stop). Do **not** start the `ModelRequest` frozen-contract bump or `Par`
   concurrency without an explicit go-ahead; both are reviewed compatibility / architecture
   events.

## Open decisions (ask the user — don't guess)

- **`ModelRequest` frozen-contract extension** (seed/max_tokens/usage + possible
  function-calling). A deliberate `NATIVE_REQUEST_SCHEMA_VERSION` 1→2 bump that ripples
  into `ModelFingerprint`/epochs/reproduction packets + the journal golden. Do it only on
  explicit go-ahead; it is a separate unit from the taxonomy above.
- **True `Par` concurrency** — needs tokio (breaks dependency-light) or deterministic
  journal-segment splicing. Architecture decision.
- **CAPES research + eval protocol** (P5 target) — larger, research-flavoured.
- The roadmap/TR-1 gate is **complete at P5 with no waivers**, so beyond the taxonomy
  everything is genuinely optional/forward-looking — the user's priority call.
- Also still offered: Romanian grant research (PoCIDIF, needs MySMIS/MIPE); a one-page
  scientific framing (C2+C3).

## How to work here (durable invariants are in CLAUDE.md)

- Build/verify with `-j 2` (cold `--workspace` OOM-kills, exit 137). Poll CPU/RAM before
  any `--workspace` run — this machine sits bursty near the 70% CPU ceiling and RAM
  crossed its 85% ceiling this session. Run gate commands unpiped, read
  `${PIPESTATUS[0]}`; foreground `sleep` is blocked (background `until`-loop / Monitor).
  Control Room emits live pressure notices — adapt.
- **Environment quirks observed this session** (cost round-trips — avoid them):
  - Shell **redirection/heredocs are DENIED** (Control Room "Dangerous system commands":
    `cat > f <<EOF` was blocked). Build commit messages with `git commit -m '...'`
    single-quoted paragraphs using **no apostrophes or backticks** (single quotes make
    backticks literal, but an apostrophe ends the quote). Use ASCII `->`/`=>` in messages.
  - The **Write tool cannot write outside the project** (Architecture Guardian). Scratchpad
    files need Bash `cp`; keep any temp message/backup files in the project or scratchpad
    via `cp`.
  - **`git add -p` is interactive → unavailable.** To split a shared file (e.g. CHANGELOG)
    across commits: `cp` it to a backup, Edit out the second unit's hunk, stage+commit the
    first, `cp` the backup back, stage+commit the second. (Verify losslessly: the per-unit
    insertion counts should sum to the whole-file insertion count.)
- `gh` is NOT authenticated: push over SSH (`git@github.com:mi7plus/entelechy.git`);
  GitHub reads via unauthenticated `curl https://api.github.com/repos/mi7plus/entelechy/...`.
- Windows + git-bash; `git status` + 0-byte inspect before every commit.
- Frozen contracts (`entelechy_contracts::FROZEN_CONTRACTS`): `entelechy.journal` (v2),
  `entelechy.model-request` (`ModelRequest`/`ProviderIdentity`, v1 — recorded in the
  journal golden AND feeding `ModelFingerprint`), canonical-serialization, design,
  failure-ontology, protocol. `CompiledPlan`/`CompiledStep`, approval signatures, and
  **`ToolError`/`ToolGateway`** are NOT frozen (the taxonomy above is safe).
- The codegen backend reuses the interpreter's helpers + `authorize_*`, so denials carry
  identical reasons on both backends; the interpreter stays the Q10 reference executor.
- TR-1 gate: `entelechy requirements --check`; `CURRENT_GATE_PHASE = P5`. Drills:
  `entelechy drill --kind restore|rollback|audit|slo|drift|degraded|rollout|enforcement|shadow|shadow-governance|all`.

## Prior units (context)

- **This session (3):** tool-effect failure taxonomy (`afa1717`) — committed, gated
  green, **NOT yet pushed**.
- **Earlier this run (pushed):** codegen conformance (`ea9e06e`), Ed25519 opt-in
  (`e810655`), handoffs (`868462a`, `c170a66`).
- **Earlier:** WP2 shadow→governance bridge (`021b8f8`) + handoff `07e0a57`; WP2 runtime
  shadow write-interception (`6bfe132`); WP1 remainder (`3316c2f`). WP1/WP2, the governance
  bridge, staged-rollout machinery, and the TR-1 advance to P5 are complete, green and
  pushed.

## Background work at handoff

**None running.** No dev server, watcher, build or test in flight. **One commit pending
push:** `afa1717` (taxonomy) is ahead of `origin/main` and waits on a user push decision.
