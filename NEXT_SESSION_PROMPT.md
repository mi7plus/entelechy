# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, **only when the user asks** (handoff docs
are the exception — see CLAUDE.md).

## Last unit (committed `2c92106` + pushed) — real_study converges onto the searched repair

`real_study` now converges onto the **searched** repair: the last hand-authored
repair has left the live study path. This is the NEXT UNIT the previous handoff
scoped, now implemented, fully verified, **committed as `2c92106` and pushed to
`main`** (this handoff refresh sits on top). The full gate was re-run green before
the commit (numbers below).

### What was built

The real-domain study (`entelechy study --real`) used to evaluate a hand-authored
`repaired_design()` (a `Verify` + a `resolve_part` committee). Now that the unified
loop *discovers* an equivalent design by execution, the study consumes the searched
design instead, and proves it is a drop-in replacement:

- **`discovered_repair()`** (`crates/entelechy-domain/src/searched.rs`, re-exported
  from `lib.rs`) — a thin, deterministic wrapper that runs the exact repair loop over
  `searched_suite()` + `baseline_design()` and returns `run_repair_loop(...).best`:
  the baseline plus the three executor-confirmed repairs (`Verify` L3, `Retrieval`
  L2, `resolve_all` `Tool` L5). +1 test (`discovered_repair_is_the_design_the_study_converges_on`).
- **`real_study()` consumes it** (`crates/entelechy-domain/src/study.rs`) — the
  candidate is now `discovered_repair()`; `repaired_design()` is retained **only** as
  a reference oracle. A new `discovered_matches_handauthored` invariant asserts that
  for every ticket in the shared two-class `helpdesk_suite`, the searched design
  resolves exactly what the hand-authored repair did (its `Retrieval` is inert where
  no ticket needs it). Folded into `ok`. +1 test
  (`the_live_repair_is_the_searched_design_not_the_hand_authored_one`) + a
  `real_study_is_green` assertion.
- **CLI** (`crates/entelechy-cli/src/study.rs`) — `print_real_study` gains a
  *Searched-repair convergence* section; `--out` persists the discovered design as
  `discovered-design.json` (its content address = the reproduction packet's sealed
  `candidate_id`) in place of `repaired-design.json`.
- **Docs + CHANGELOG** — `lib.rs`/`study.rs` module docs, `real_study`,
  `repaired_design` (now "reference oracle"), the new report field, and a new
  CHANGELOG entry.

Because the discovered design resolves the two-class suite identically, every
count-asserting invariant is unchanged: comparator `0/18 → 18/18`; PV-2 release
`3/6` vs pooled `9/12`; reproduction `b=0 c=30`; holdout `pass`. The PV-2 drift
narrative is preserved (the discovered design still grounds via a `Verify`). So the
one Phase-0 loop now discovers **every** repair the studies rely on — no
hand-authored repair remains on a live study path.

### Files in that commit (`2c92106`)

- `crates/entelechy-domain/src/searched.rs` (`discovered_repair()` + 1 test),
- `crates/entelechy-domain/src/study.rs` (consume discovered repair; new invariant +
  field + 2 tests; docs),
- `crates/entelechy-domain/src/lib.rs` (re-export `discovered_repair`; module doc),
- `crates/entelechy-cli/src/study.rs` (convergence narrative; persist
  `discovered-design.json`; docs),
- `CHANGELOG.md`. (This handoff refresh is a separate commit.)

### Verified state (this session, all green)

- `cargo fmt --all --check` — clean
- `cargo clippy --workspace --all-targets -- -D warnings` — clean
- `cargo clippy -p entelechy-gateway --all-targets --features wasm,openai-tls -- -D warnings` — clean
- `cargo test --workspace -j 2` — **352 passed, 0 failed** (was 350; +2 tests)
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` — clean
- `cargo run -p entelechy-cli -- study --real` — prints `SATISFIED`; the convergence
  section shows `drop-in for hand-authored: yes`; counts unchanged (0/18→18/18,
  release 3/6 vs pooled 9/12, reproduced b=0 c=30), holdout `pass`
- `cargo run -p entelechy-cli -- study --real --out <dir>` — persists
  `discovered-design.json` (6 nodes: root, draft_retrieve/Retrieval, draft,
  resolve-first, draft_check/Verify, draft_resolve_all/resolve_all)
- `cargo run -p entelechy-cli -- study --searched` — still `SATISFIED`, accepted
  levels `[3, 2, 5]`
- `cargo run -p entelechy-cli -- study` (simulated) — unchanged
- `cargo run -p entelechy-cli -- requirements --check --phase p4` — **PASS**

Build with `-j 2` (a cold `--workspace` build OOM-kills, exit 137). Note: the
recurring git-bash stray 0-byte files (`1`, `bool`, `u32`, `usize`,
`ComparisonReport`, `self.reply.as_ref().is_some_and(`) reappeared from unquoted
`grep`/`find` args this session and were deleted — `git status` is clean of them.
Quote args containing `()><` and prefer `git commit -F -` with a quoted heredoc.

## NEXT UNIT (to scope) — beyond the now-unified Phase-0 study trilogy

With this unit, all three studies are unified and **every** repair on a live study
path is discovered by execution, not hand-authored. Candidate next units:

1. **`restudy.rs` still hand-authors its repair.** `crates/entelechy-cli/src/restudy.rs`
   has its own local `repaired_design()` (distinct from the domain one). If that
   command should also demonstrate a searched repair, it could consume
   `entelechy_domain::discovered_repair()` the same way — the smallest follow-on in
   this theme. (Check first whether `restudy`'s narrative *wants* a hand-authored
   contrast, as the simulated `study` deliberately keeps its structure-scorer.)
2. **Advance the gate past P4.** `CURRENT_GATE_PHASE = P4` (committed `2ac6002`). P5
   is the only phase with an unrecorded requirement, **PC-3** (plugin registry, first
   enforced at P5). Advancing means recording PC-3 in
   `entelechy_contracts::traceability` with evidence (or an active waiver) and bumping
   `CURRENT_GATE_PHASE` — a self-contained gate unit.
3. **P4/P5 features** — shadow/canary rollout, web console, SDKs (P4/P3);
   multi-tenancy, plugin registry, learned priors, distillation (P5). Larger; not
   implemented.

Recommend scoping (1) if a searched `restudy` is wanted, else (2) to move the gate
forward. The simulated `entelechy study` structure-scorer stays as the honest
contrast — do not "converge" it.

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
  loop — the deliberate honest contrast), `entelechy study --real` (real-domain,
  executor-scored — now consumes the **searched** repair via `discovered_repair()`),
  and `entelechy study --searched` (the repair loop scored by execution — discovers
  all three repairs: reply/verify L3, knowledge/retrieval L2, decomposition/resolve_all L5).

The CLI surface: `init, objective, capability, eval, design, study [--real]
[--searched] [--out], trace, replay, counterfactual, diff, assure, release, serve,
committee, audit, demo, ops, drill [--kind restore|rollback|audit|slo|drift|degraded|all],
quickstart [--out], requirements [--check [--phase pN]]` (+ `infer` under
`--features openai`).
