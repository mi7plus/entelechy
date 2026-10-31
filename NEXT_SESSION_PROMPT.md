# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, **only when the user asks** (handoff docs
are the exception — see CLAUDE.md).

## Last unit (committed + pushed) — the searched loop discovers the decomposition repair

Follow-on #1 ("finish unifying the studies — the searched loop should discover the
**decomposition** repair by execution, not just the reply one") is **committed as
`dfb4e86` and pushed to `main`** (this handoff refresh sits on top). The full gate was
re-run green before the commit (numbers below).

### What was built

The searched study (`entelechy study --searched`) used to discover only the
reply-class fix; the decomposition class still leaned on the hand-authored
`resolve_part` committee in `real_study`, because the proposer's level-4 operator
emits a reasoning committee of `Llm` sub-agents that resolve nothing when executed.
This closes that gap, so **both** Phase-0 repairs are now searched, not hand-authored:

- **New edit operator** `entelechy_design::EditOp::AddTool { id, capability, args }`
  (`crates/entelechy-design/src/edit.rs`) — appends a `Tool` (acting) node to the
  root `Seq`, the smallest level-1 change for a repair whose fix is a capability call
  rather than more reasoning. Fails closed if the root is not a `Seq`.
- **Proposer wiring** (`crates/entelechy-search/src/proposer.rs`) — `ops_for_level`
  is now class-aware: for `reasoning.decomposition`, level 5 (the coordination tier)
  is realized as `AddTool { capability: "resolve_all" }` instead of a bare delegate.
  It invokes a capability the agent already holds, so it widens no authority (IR-I2);
  `propose`'s signature is unchanged (it already took `class_id`).
- **Authority** (`crates/entelechy-domain/src/study.rs`) — `authority()` now grants
  `resolve_all` (granted but unused by the baseline, which resolves only the first
  part), so the discovered `resolve_all` tool actually resolves when executed.
- **Searched study** (`crates/entelechy-domain/src/searched.rs`) — `searched_study()`
  gains two invariants, `discovered_part_resolver` and
  `discovered_resolves_decomposition`, folded into `ok`. The loop now proposes the
  level-4 reasoning committee (executor **rejects** it — the honest-divergence
  contrast is intact: a structural scorer would credit its `Par`), then escalates and
  **discovers** the `resolve_all` tool at level 5, which the executor confirms
  resolves every part. `accepted_levels` is now `[3, 5]`.

The two lessons land in one run: more reasoning agents (the committee) resolve
nothing here, while a humble acting step the agent was already authorized for does.

### Files in that commit (`dfb4e86`)

- `crates/entelechy-design/src/edit.rs` (`AddTool` op + 2 tests),
  `crates/entelechy-search/src/proposer.rs` (class-aware level 5 + 1 test),
  `crates/entelechy-domain/src/study.rs` (grant `resolve_all`),
  `crates/entelechy-domain/src/searched.rs` (2 new invariants + docs + updated tests),
  `crates/entelechy-cli/src/study.rs` (`print_searched_study` lines + doc),
  `CHANGELOG.md`. (This handoff refresh is a separate commit.)
- Note: three stray 0-byte files (`1`, `AuthorityEnvelope`,
  `self.reply.as_ref().is_some_and(`) recurred from git-bash grep/awk args with
  unquoted `()` and were deleted before the commit — quote such args, and prefer
  `git commit -F -` with a quoted heredoc over `-m`.

### Verified state (this session, all green)

- `cargo fmt --all --check` — clean
- `cargo clippy --workspace --all-targets -- -D warnings` — clean
- `cargo clippy -p entelechy-gateway --all-targets --features wasm,openai-tls -- -D warnings` — clean
- `cargo test --workspace -j 2` — **347 passed, 0 failed** (was 344; +3 tests)
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` — clean
- `cargo run -p entelechy-cli -- requirements --check --phase p4` — **PASS** (gate untouched)
- `cargo run -p entelechy-cli -- study --searched` — prints `SATISFIED`; shows
  `accepted levels: [3, 5]`, `discovered a resolver: yes`, `executor resolves parts: yes`
- `cargo run -p entelechy-cli -- study` (simulated) and `study --real` — still pass

Build with `-j 2` (a cold `--workspace` build OOM-kills, exit 137).

## Follow-ons (next natural units)

The gate is at the project's furthest enforced phase (`CURRENT_GATE_PHASE = P4`,
committed as `2ac6002`). Natural follow-ons:

1. **Exercise the level-2 retrieval path end to end** (add a retrieval-miss failure
   class so a run visibly unlocks retrieval at level 2; note the executor currently
   treats a `Retrieval` node as inert, so this needs a retrieval effect in the world
   too — mirror of what `AddTool`/`resolve_all` just did for decomposition).
2. **Optional: converge `real_study` onto the searched repair.** `real_study` still
   hand-authors `repaired_design` (the `resolve_part` committee + verify). Now that
   the loop discovers an equivalent design by execution, `real_study` could consume
   the discovered design instead of the hand-authored one (or assert they resolve the
   same tickets) to retire the last hand-authored repair.
3. **Shadow/canary rollout, web console, SDKs (P4/P3)** — not implemented.
4. **P5**: multi-tenancy, plugin registry (PC-3, the only unrecorded requirement —
   first enforced at P5), learned priors, distillation. Advancing the gate to P5
   means recording PC-3 and bumping `CURRENT_GATE_PHASE`.

## How to work here (see CLAUDE.md for durable invariants)

- Build/test with `-j 2` — a cold `--workspace` build OOM-kills (exit 137).
- `gh` is NOT authenticated: push over the SSH remote
  (`git@github.com:mi7plus/entelechy.git`); for PR/CI status use unauthenticated
  `curl https://api.github.com/repos/mi7plus/entelechy/...`.
- Windows + git-bash; `git status` and inspect before every commit (stray 0-byte
  files recur from shell quoting — quote grep/command args containing `()><`, and
  prefer `git commit -F -` with a quoted heredoc over `-m` for long messages).
- Conventions: each crate's `//!` cites its PRD section; commit messages reference
  requirement IDs / PRD sections; heavy engines stay behind features (`wasm`,
  `openai`, `openai-tls`).
- TR-1 phase gate is `entelechy requirements --check`; evidence lives in
  `entelechy_contracts::phase_gate_evidence()` with `CURRENT_GATE_PHASE`.
- The three Phase-0 studies: `entelechy study` (simulated, structure-scored repair
  loop), `entelechy study --real` (real-domain, executor-scored hand-authored repair),
  and `entelechy study --searched` (the repair loop scored by execution — the
  unification; now discovers **both** the reply and decomposition repairs).

The CLI surface: `init, objective, capability, eval, design, study [--real]
[--searched] [--out], trace, replay, counterfactual, diff, assure, release, serve,
committee, audit, demo, ops, drill [--kind restore|rollback|audit|slo|drift|degraded|all],
quickstart [--out], requirements [--check [--phase pN]]` (+ `infer` under
`--features openai`).
