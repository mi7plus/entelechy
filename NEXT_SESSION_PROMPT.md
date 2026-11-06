# Next session — Entelechy

_Handoff finalized 2026-11-07 (Control Room run #10, session 2 → context cleared at
the Autopilot threshold). Two units shipped to `main`; the next unit is **fully
designed and ready to implement** (spec below) but not yet written._

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push to
`main` after each self-contained unit, **only when the user asks** (handoff docs are
the exception — see CLAUDE.md).

> **`HEAD = origin/main = 868462a`. Working tree is CLEAN — nothing uncommitted.**
> No code was written for the next unit (the context hit the handoff threshold during
> design, under resource pressure), so you start from a clean tree with a complete spec.

## State

Shipped and pushed this run (all green):
- `ea9e06e` — codegen conformance backend (control-flow + budget/egress + Gate/Tool/
  Delegate lowering). PRD §21 Phase 4, Q10.
- `e810655` — Ed25519 approval signatures behind the opt-in `ed25519` feature. PRD 5.9,
  AU-2. Kept **opt-in** (dependency-light default preserved).
- `868462a` — handoff-doc refresh.

Verification (this session, against the final committed code):
- `cargo fmt --all --check` — clean.
- `cargo test -p entelechy-runtime -j 2` — exit 0 (codegen conformance + golden + wp1 +
  wp2).
- `cargo test -p entelechy-identity --features ed25519 -j 2` — exit 0 (18 tests, incl. 4
  crypto-path).
- CHANGELOG split confirmed correct (4 `### Added` entries; Ed25519 hunk in `e810655`,
  codegen hunks in `ea9e06e`).
- The **full workspace gate** (clippy `-D warnings` incl. `wasm,openai-tls` and `ed25519`
  feature paths, `cargo test --workspace`, rustdoc `-D warnings`, `requirements --check`
  PASS @ P5, every drill SATISFIED) was green at the previous handoff against
  byte-identical code; **not re-run this session** (code unchanged; RAM sat over its 85%
  ceiling). Re-run it once you write code.

## START HERE — the next unit (designed, ready to implement)

**Tool-effect failure taxonomy: distinguish a definitive rejection (nothing committed →
clean `Failed`) from a lost/indeterminate acknowledgement (possible effect →
`ReconciliationRequired`). PRD 8.5, 8.6; Q10 parity.**

This delivers the concrete value the "full gateway contract" was wanted for. The gap is
flagged in-code at `crates/entelechy-runtime/src/exec.rs` (Execute `Tool` arm): a failed
`tools.call` currently always yields `ReconciliationRequired` — the comment says
distinguishing a clean rejection "needs the richer gateway contract deferred to WP2".

**Why this scope, not a `ModelRequest` bump:** `ToolError`/`ToolGateway` (`tool.rs`) are
**NOT** in `entelechy_contracts::FROZEN_CONTRACTS`, so this needs **no frozen-contract
bump**. The alternative framing ("usage/token-limits/seed on `ModelRequest`") would bump
`NATIVE_REQUEST_SCHEMA_VERSION` 1→2, which ripples into the content-addressed
`ModelFingerprint` (recorded `request_schema_version`), drift epochs and the domain
study's reproduction packets + the journal golden — a reviewed compatibility event. Hold
that for an explicit user decision (see "Open decisions").

### Exact edits

1. **`crates/entelechy-gateway/src/tool.rs`**
   - Add `pub enum EffectOutcome { NoEffect, Indeterminate }` (derive
     `Clone,Copy,Debug,PartialEq,Eq`) with a doc citing PRD 8.5/8.6.
   - Add variant `ToolError::Rejected(String, String)` (`#[error("tool '{0}' rejected the
     call: {1}")]`) between `UnknownCapability` and `Failed`.
   - Add `impl ToolError { pub fn effect_outcome(&self) -> EffectOutcome { ... } }`:
     `UnknownCapability | Rejected => NoEffect`; `Failed => Indeterminate` (the
     conservative default — an app-level error may follow a partial effect).
   - Change the field to `tools: HashMap<String, (NativeTool, bool)>` (the `bool` is
     "guarded"). `#[derive(Default)]` still holds.
   - `register(...)` inserts `(Box::new(tool), false)`; add `register_guarded(...)`
     (same signature) inserting `(Box::new(tool), true)` — a guarded tool validates and
     may refuse **before** any side effect, so its `Err` is `Rejected` (NoEffect).
   - `call(...)`: `let entry = self.tools.get_mut(&call.capability).ok_or_else(|| ToolError::UnknownCapability(...))?; let guarded = entry.1; let tool = &mut entry.0;` then map `Err(e)` → `if guarded { Rejected } else { Failed }`.
   - Tests: `guarded_tool_rejection_is_no_effect`, `plain_tool_error_is_indeterminate`,
     `unknown_capability_is_no_effect`.
2. **`crates/entelechy-gateway/src/lib.rs`** — add `EffectOutcome` to
   `pub use tool::{...}`.
3. **`crates/entelechy-runtime/src/exec.rs`**
   - Import: `use entelechy_gateway::{EffectOutcome, ModelRequest, ToolCall};`
   - Execute `Tool` arm `Err` branch (~608–616): replace `Err(_) => ReconciliationRequired`
     with `Err(e) => match e.effect_outcome()`:
     - `NoEffect` → `journal.entries.truncate(intent);` (drop the speculative write-ahead
       intent — nothing committed, nothing to reconcile) then
       `Err(Halt::Terminal(RunStatus::Failed { node_path: path.to_string(), reason: FailureReason::ToolError(e.to_string()) }))`.
     - `Indeterminate` → `Err(Halt::Terminal(RunStatus::ReconciliationRequired))` (leave the
       Unknown intent dangling — unchanged behavior).
     - Update the comment (remove "deferred to WP2"). `intent` is captured at the
       `let intent = journal.entries.len();` just before the pre-dispatch append, so
       `truncate(intent)` removes exactly that one entry.
   - Leave the `Counterfactual` `Tool` arm as-is (analysis mode, no write-ahead intent).
4. **`crates/entelechy-runtime/src/codegen.rs`** (MUST mirror for Q10 parity —
   `observable_equivalent` compares `status`+`output`)
   - `Tool` arm `Err` branch (~633–640): `Err(e) => match e.effect_outcome()`:
     `entelechy_gateway::EffectOutcome::NoEffect => Err(RunStatus::Failed { node_path, reason: FailureReason::ToolError(e.to_string()) })`;
     `Indeterminate => Err(RunStatus::ReconciliationRequired)`. (`node_path` is already
     computed above as `format!("{base}{path}")`; the compiled backend journals no intent,
     so nothing to truncate.)
   - Update the `CompiledStep::Tool` doc (~173) "unacknowledged dispatch forces
     ReconciliationRequired" to note the taxonomy.
   - Tests (module at bottom): in `register_tools` add
     `tools.register_guarded("validate_amount", |_| Err("amount out of range".into()));`
     add `reject_tool_program()` (a `Seq` with one `Tool` id `"check"`, capability
     `"validate_amount"`, authority granting `validate_amount`); add `tool_rejection_conforms`
     — `run_both(&reject_tool_program(), ..., None, || None)`, assert interp is
     `Failed { node_path == "root/0:check", reason: ToolError(_) }`,
     `observable_equivalent(&interp,&codegen)`, and `!interp.journal.has_unknown_commit()`.
5. **`crates/entelechy-runtime/tests/wp1_enforcement.rs`** (interpreter red-team; helpers
   `auth(&[..])`, `tool(id, cap)` exist)
   - `unknown_capability_fails_cleanly_not_reconciliation`: `auth(&["missing_tool"])`,
     `tool("x","missing_tool")`, do **not** register it → status `Failed` (ToolError),
     `!run.journal.has_unknown_commit()`, `run.output.is_none()`.
   - `guarded_rejection_fails_cleanly`: register a guarded tool that `Err`s, grant it →
     `Failed`, no dangling intent.
   - The existing `tool_intent_is_journaled_before_dispatch` /
     `crash_between_dispatch_and_commit_is_recoverable` use plain `register` (→ `Failed` →
     Indeterminate) and MUST stay `ReconciliationRequired` (regression guard — do not
     break them).
6. **Docs/CHANGELOG**: new `### Added` CHANGELOG entry (PRD 8.5/8.6, Q10 parity, non-frozen,
   no new dep); update the reconciliation wording in `docs/wp1-correctness-gate.md`
   (the WS5 section) and the `ReconciliationRequired` mention in `ARCHITECTURE.md:~83` to
   note the taxonomy.
7. **Gate** (full workspace, `-j 2`, poll pressure first) then **commit as one unit**
   (reference PRD 8.5/8.6; `Co-Authored-By: Claude Opus 4.8`), **push only when the user
   asks**.

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

## Prior units (committed + pushed — context)

- **This run:** codegen conformance (`ea9e06e`), Ed25519 opt-in (`e810655`), handoff
  (`868462a`).
- **Earlier:** WP2 shadow→governance bridge (`021b8f8`) + handoff `07e0a57`; WP2 runtime
  shadow write-interception (`6bfe132`); WP1 remainder (`3316c2f`). WP1/WP2, the governance
  bridge, staged-rollout machinery, and the TR-1 advance to P5 are complete, green and
  pushed.

## Background work at handoff

**None running.** No dev server, watcher, build or test in flight. No commits pending.
