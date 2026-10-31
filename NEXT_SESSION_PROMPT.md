# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, **only when the user asks** (handoff docs
are the exception — see CLAUDE.md).

## Last unit (committed `b52532d` + pushed) — level-2 retrieval discovered end to end

The searched loop now discovers the **level-2 retrieval** repair by execution, not
just the reply (level 3) and decomposition (level 5) ones. This is follow-on #1 from
the previous handoff ("exercise the level-2 retrieval path end to end"). It is
**committed as `b52532d` and pushed to `main`** (this handoff refresh sits on top).
The full gate was re-run green before the commit (numbers below).

### What was built

The level-2 *vocabulary* already existed (the `AddRetrieval` operator, the proposer's
level-2 emission, and the `knowledge.retrieval_miss` class eligible at `[2]`), but no
run exercised it. Three gaps are closed, mirroring the `AddTool`/`resolve_all`
decomposition unit (`dfb4e86`):

- **World effect** (`crates/entelechy-domain/src/world.rs`) — a new
  `TicketKind::Knowledge` (`needs_retrieval`), resolved only once a reply is drafted
  **and** the needed durable fact has been retrieved; a `retrieve(tier, top_k)` effect
  that surfaces the fact only from the **semantic** tier with `top_k >= 1` (a
  misconfigured retrieval finds nothing — the teeth). `resolved()`/`reset()`/`new()`
  updated; `retrieved_fact()` accessor added. +2 unit tests.
- **Executor** (`crates/entelechy-domain/src/exec.rs`) — `apply` now honors
  `NodeKind::Retrieval` by calling `world.retrieve(r.tier, r.top_k)` (a read; adds no
  authority). It was previously inert (`_ => {}`). +1 unit test.
- **Per-class level cursor** (`crates/entelechy-domain/src/searched.rs`) — the repair
  loop tracked one monotonic `level`; now it tracks the level **per failure class**
  (`BTreeMap<String, u8>`) and, each round, picks the largest actionable cluster whose
  class can still escalate. This honors PRD §11.4 ("levels are a prior, not a strict
  ladder"): a level-2 retrieval miss is now discovered *after* a level-3 verify, and a
  stuck/approval-gated class no longer strands the others.
- **Own three-class suite** — `searched_study` now uses a local `searched_suite()`
  (tune 7 reply / 6 knowledge / 5 decomposition; validation 4/4/4; holdout 7/6/5),
  leaving `real_study`'s shared `helpdesk_suite` and its PV-2 drift narrative
  untouched. Validation is equal-thirds so each repair clears the rolling-validation
  bar independently (4 of 12 new wins → paired-bootstrap lower bound > 0; 3 of 12 would
  sit on the 2.5% boundary). Tune counts order discovery (reply > knowledge > dec).
- **New invariants** — `baseline_knowledge_unresolved`, `discovered_retrieval`,
  `discovered_resolves_knowledge`, folded into `ok`. `accepted_levels` is now
  `[3, 2, 5]` (deliberately not level-ordered). CLI `print_searched_study` reports the
  retrieval lines and the non-ladder note.

So the one loop now discovers all three Phase-0 repairs by execution — verify (3),
retrieval (2, found *below* the verify level), resolve_all (5) — and still rejects the
level-4 reasoning committee the structural scorer would credit (honest divergence
intact).

### Files in that commit (`b52532d`)

- `crates/entelechy-domain/src/world.rs` (`Knowledge` kind + `retrieve` + 2 tests),
- `crates/entelechy-domain/src/exec.rs` (honor `Retrieval` + 1 test),
- `crates/entelechy-domain/src/searched.rs` (per-class cursor, `searched_suite`, 3 new
  invariants, module docs, updated tests, contract split sizes 18/12/18),
- `crates/entelechy-cli/src/study.rs` (retrieval narrative lines + doc),
- `CHANGELOG.md`. (This handoff refresh is a separate commit.)
- Note: three stray 0-byte files (`1`, `AuthorityEnvelope`,
  `self.reply.as_ref().is_some_and(`) recurred again from git-bash args with unquoted
  `()` and were deleted before the commit — quote such args, and prefer
  `git commit -F -` with a quoted heredoc over `-m`.

### Verified state (this session, all green)

- `cargo fmt --all --check` — clean
- `cargo clippy --workspace --all-targets -- -D warnings` — clean
- `cargo clippy -p entelechy-gateway --all-targets --features wasm,openai-tls -- -D warnings` — clean
- `cargo test --workspace -j 2` — **350 passed, 0 failed** (was 347; +3 tests)
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` — clean
- `cargo run -p entelechy-cli -- study --searched` — prints `SATISFIED`; shows
  `accepted levels: [3, 2, 5]`, discovered verify/retrieval/resolver all `yes`,
  knowledge resolved, honest divergence held, holdout `pass`
- `cargo run -p entelechy-cli -- study` (simulated) and `study --real` — still pass
- `cargo run -p entelechy-cli -- requirements --check --phase p4` — **PASS** (gate untouched)

Build with `-j 2` (a cold `--workspace` build OOM-kills, exit 137).

## NEXT UNIT (scoped, ready to implement) — converge `real_study` onto the searched repair

`real_study` (`crates/entelechy-domain/src/study.rs`) still hand-authors its candidate
`repaired_design()` (the `resolve_part` committee + verify). Now that the searched loop
discovers an equivalent design by execution, retire that hand-authored repair from the
study's live path: `real_study` should consume the *discovered* design and prove it is a
drop-in replacement. Concrete plan (derived and de-risked this session):

1. **Expose the discovered repair.** Add `pub fn discovered_repair() -> Program` to
   `crates/entelechy-domain/src/searched.rs` — a thin wrapper that builds
   `searched_suite()` + `baseline_design()` and returns `run_repair_loop(...).best`.
   Re-export it from `crates/entelechy-domain/src/lib.rs`.
2. **`real_study` consumes it.** In `real_study()`, set `let repaired = discovered_repair();`
   (replacing `repaired_design()`) so the candidate, comparison, PV-2 epochs, holdout gate
   and reproduction packet all use the searched design.
3. **Assert equivalence to the retired repair.** Keep `repaired_design()` as a reference
   oracle. Add a `discovered_matches_handauthored: bool` invariant to `RealStudyReport`:
   for every task in `helpdesk_suite()`, `resolves(&discovered, t, &gw) ==
   resolves(&handauthored, t, &gw)`. Fold it into `ok`. This proves the searched repair is
   a drop-in replacement while demonstrating no hand-authored repair is needed.
4. **CLI** (`crates/entelechy-cli/src/study.rs`): add a convergence line to
   `print_real_study`; in `run_real`, persist the discovered design (e.g.
   `discovered-design.json`) instead of the hand-authored one, and update the doc comment.
5. **CHANGELOG** entry; then full gate + commit (ask the user before committing/pushing —
   CLAUDE.md).

**Keep `real_study` on the 2-class `helpdesk_suite`** (do NOT unify suites). Verified this
session: on `helpdesk_suite` the discovered design resolves reply (verify) and
decomposition (`resolve_first` + `resolve_all`) identically to the committee, and its
`Retrieval` node is inert there — so every count-asserting test stays green unchanged
(`release_evidence_is_the_post_drift_epoch_not_the_pool` keeps `release 3/6`, `pooled
9/12`; the drift narrative is preserved because the discovered design still has a `Verify`).
Unifying suites would churn those counts for little gain; leave it.

## Other follow-ons

The gate is at the project's furthest enforced phase (`CURRENT_GATE_PHASE = P4`,
committed as `2ac6002`).

- **Shadow/canary rollout, web console, SDKs (P4/P3)** — not implemented.
- **P5**: multi-tenancy, plugin registry (PC-3, the only unrecorded requirement — first
  enforced at P5), learned priors, distillation. Advancing the gate to P5 means recording
  PC-3 and bumping `CURRENT_GATE_PHASE`.

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
  unification; now discovers **all three** repairs: reply, knowledge/retrieval, and
  decomposition).

The CLI surface: `init, objective, capability, eval, design, study [--real]
[--searched] [--out], trace, replay, counterfactual, diff, assure, release, serve,
committee, audit, demo, ops, drill [--kind restore|rollback|audit|slo|drift|degraded|all],
quickstart [--out], requirements [--check [--phase pN]]` (+ `infer` under
`--features openai`).
