# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, **only when the user asks** (handoff docs
are the exception — see CLAUDE.md).

## Last unit (committed + pushed) — the two Phase-0 studies are unified

The "unify the two studies / drive repair through the real proposer loop" follow-on
is **committed as `c05b097` and pushed to `main`** (this handoff refresh sits on top).
The full gate was re-run green before the commit:

- `cargo fmt --all --check` — clean
- `cargo clippy --workspace --all-targets -- -D warnings` — clean
- `cargo clippy -p entelechy-gateway --all-targets --features wasm,openai-tls -- -D warnings` — clean
- `cargo test --workspace -j 2` — **344 passed, 0 failed**
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` — clean
- `cargo run -p entelechy-cli -- requirements --check --phase p4` — **PASS**
- `cargo run -p entelechy-cli -- study --searched` — prints **SATISFIED**

### What it did

A new `entelechy-domain::searched` module (`searched_study()`), surfaced as
`entelechy study --searched [--out]`. It drives the **same** design-repair proposer
loop the simulated `entelechy study` uses — `entelechy_failure::analyze` →
`entelechy_search::propose` → `entelechy_design::apply` →
`rolling_validation_decision` — but scores every candidate by **executing** it against
the resettable helpdesk world (`exec::resolves`) instead of by reading its structure.

The honest unification of the two Phase-0 studies:

- the loop **discovers** (does not hand-author) the verification repair for the reply
  class, accepts it at level 3, and the executor confirms it resolves the reply
  tickets the baseline failed;
- the level-4 parallel committee it proposes for the decomposition class is
  **rejected by execution** (running it resolves nothing — the committee's `Llm`
  sub-agents carry no part-resolving effect in this world), **even though the
  structure-reading scorer would have credited it** (it "has a `Par`"). Same search,
  two scorers, and execution is the honest one.

Files in that commit: new `crates/entelechy-domain/src/searched.rs`; modified
`crates/entelechy-domain/Cargo.toml` (adds `entelechy-{failure,design,search}` deps —
no cycle; the CLI already used all three), `crates/entelechy-domain/src/lib.rs`,
`crates/entelechy-cli/src/main.rs` (`--searched` arg + dispatch),
`crates/entelechy-cli/src/study.rs` (`run(out, real, searched)` + `run_searched` +
`print_searched_study`), `CHANGELOG.md`, `Cargo.lock`.

## Housekeeping note (not part of any unit)

`fuzz/Cargo.lock` is an untracked, un-ignored lockfile left over from an earlier
fuzz build; it is **not** tracked and was deliberately kept out of the `c05b097`
commit. Decide separately whether the cargo-fuzz sub-crate's lockfile should be
committed or git-ignored (`fuzz/target/` is already ignored).

## Follow-ons (next natural units)

The gate is at the project's furthest enforced phase (`CURRENT_GATE_PHASE = P4`,
committed as `2ac6002`). Natural follow-ons:

1. **Finish unifying the studies (last unit covered the reply class).** The
   searched loop discovers the reply-class repair through execution, but the
   decomposition class still resolves only via the hand-authored `resolve_part`
   committee in `real_study` — the generic proposer's level-4 operator emits `Llm`
   sub-agents, which have no part-resolving effect in this world. Closing the gap
   needs a **part-resolving edit operator** in the `entelechy-design` vocabulary (e.g.
   an `AddTool`/`resolve_all` operator) so the loop can *discover* the decomposition
   repair by executing, not just the reply one. That is the remaining half of "drive
   repair through the real proposer loop."
2. **Exercise the level-2 retrieval path end to end** (add a retrieval-miss failure
   class so a run visibly unlocks retrieval; note the executor currently treats a
   `Retrieval` node as inert, so this needs a retrieval effect in the world too).
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
  unification).

The CLI surface: `init, objective, capability, eval, design, study [--real]
[--searched] [--out], trace, replay, counterfactual, diff, assure, release, serve,
committee, audit, demo, ops, drill [--kind restore|rollback|audit|slo|drift|degraded|all],
quickstart [--out], requirements [--check [--phase pN]]` (+ `infer` under
`--features openai`).
