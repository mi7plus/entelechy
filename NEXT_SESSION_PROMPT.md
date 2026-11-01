# Next session — Entelechy

Continue implementing the Entelechy PRD v11 (`Entelechy_PRD_v11_FINAL.docx` in
`~/Downloads`, outside the repo). Dependency-light Rust workspace; commit and push
to `main` after each self-contained unit, **only when the user asks** (handoff docs
are the exception — see CLAUDE.md).

## Where things stand — the requirements gate is MAXED

`CURRENT_GATE_PHASE = P5`, the final roadmap phase, and **every §17.6 registry
requirement is recorded and verified**. The TR-1 phase gate
(`entelechy requirements --check`) attests every normative requirement through the
last phase. Further gate advancement is not a thing — remaining work is roadmap
**features**, which do not move the gate.

**Nothing is in flight.** The last unit (PC-3) is committed and pushed. The next
unit is a *scope decision* that belongs to the user (see "Next unit" below).

## Last unit (committed `e2ada81` + pushed — DONE)

**Phase gate advanced P4 → P5 by implementing and recording PC-3** (plugin-manifest
compatibility, fail-closed loading). Committed as `e2ada81` and pushed to
`origin/main` on 2026-11-01.

PC-3 (§16.6: "plugin manifests declare compatibility; loading fails closed") was the
only unrecorded P5 registry requirement. Its compatibility rule existed as
`entelechy_protocol::PluginManifest::check_load`, **but the real loader**
`entelechy_gateway::sandbox::PluginHost::load` **never called it** — it enforced only
the no-ambient-authority capability grant, so PC-3 was a dead function. The unit
closed that gap and advanced the gate:

- **`entelechy-protocol`** (`src/lib.rs`): `check_load` now also fails closed on a
  manifest that declares no usable window (inverted `VersionRange`) via
  `LoadError::UndeclaredCompatibility`, with a `VersionRange::is_well_formed` helper.
  Added a full fail-closed conformance matrix + serde round-trip
  (`plugin_manifest_declares_compatibility_and_loading_fails_closed`). The wire
  shapes of `PluginManifest`/`HostEnvironment` are unchanged (the frozen
  `entelechy.protocol` contract holds).
- **`entelechy-gateway`** (`src/sandbox.rs`, not feature-gated — always compiled):
  `PluginHost::load` now takes the `HostEnvironment` and enforces the manifest's
  declared compatibility (PC-3) **before** the capability grant (16.3), surfacing
  `HostError::Incompatible(LoadError)`. Test
  `incompatible_manifest_fails_closed_before_authority` proves an incompatible host
  is refused even when every capability is granted.
- **`entelechy-contracts`** (`src/traceability.rs`): PC-3 recorded in
  `phase_gate_evidence()` (citing both the protocol conformance test and the gateway
  enforcement test); `CURRENT_GATE_PHASE` bumped `P4 → P5` with updated docs.
- **Docs**: `CHANGELOG.md` entry; `ARCHITECTURE.md` plugin-host bullet now notes the
  compatibility-before-authority gate.

Verified green before commit: `cargo fmt --all --check`; `cargo clippy --workspace
--all-targets -- -D warnings`; `cargo clippy -p entelechy-gateway --all-targets
--features wasm,openai-tls -- -D warnings`; `cargo test --workspace -j 2` =
**354 passed, 0 failed**; `RUSTDOCFLAGS="-D warnings" cargo doc --workspace
--no-deps`; `requirements --check` PASS at P5 and `--phase p4` PASS.

## Prior unit (committed `2c92106` + pushed, handoff `9c89eec`)

`real_study` converged onto the searched repair: it consumes
`entelechy_domain::discovered_repair()` (Verify L3 + Retrieval L2 + resolve_all L5),
proven a drop-in for the retired hand-authored `repaired_design()` via the
`discovered_matches_handauthored` invariant. No hand-authored repair remains on a
live *study* path. (`restudy.rs` keeps its own hand-authored repair — intentionally:
it is the production-ops operate→re-study→release loop, a different domain, not a
candidate for `discovered_repair()`.)

## Next unit — AWAITING THE USER'S SCOPE DECISION

The gate is maxed, so the next unit is a feature, not a registry requirement, and
the "advance the gate" through-line is complete. **The user has an open question
(inbox) choosing which feature to scope next.** Do not start building until they
pick. The candidates, from most to least self-contained:

1. **Shadow / canary rollout** (P4) — *recommended*. A staged-promotion path with
   shadow traffic and canary analysis feeding the existing release gate. Builds on
   `entelechy-release` promotion + `entelechy-ops` online eval. Most self-contained;
   clearest evidence story; extends well-tested machinery.
2. **Web console / SDKs** (P3/P4) — a read-only console over the
   audit/journal/study artifacts, or a thin client SDK over the protocol. Larger; UI
   scope.
3. **P5 ecosystem features** — multi-tenancy, a plugin *registry* (discovery/install
   over the now-enforced PC-3 manifests), learned priors, distillation. Largest.

If the user has since answered, scope the chosen unit, confirm the scope in one
line, then build it to completion under the full gate.

## How to work here (see CLAUDE.md for durable invariants)

- Build/test with `-j 2` — a cold `--workspace` build OOM-kills (exit 137).
- `gh` is NOT authenticated: push over the SSH remote
  (`git@github.com:mi7plus/entelechy.git`); for PR/CI status use unauthenticated
  `curl https://api.github.com/repos/mi7plus/entelechy/...`.
- Windows + git-bash; `git status` and inspect before every commit (stray 0-byte
  files recur from shell quoting — quote grep/command args containing `()><`, and
  prefer `git commit -F -` with a quoted heredoc over `-m` for long messages). This
  session's working tree was clean of them.
- Conventions: each crate's `//!` cites its PRD section; commit messages reference
  requirement IDs / PRD sections; heavy engines stay behind features (`wasm`,
  `openai`, `openai-tls`).
- TR-1 phase gate is `entelechy requirements --check`; evidence lives in
  `entelechy_contracts::phase_gate_evidence()` with `CURRENT_GATE_PHASE` (now P5).
- The three Phase-0 studies: `entelechy study` (simulated, structure-scored — the
  deliberate honest contrast), `entelechy study --real` (real-domain, executor-
  scored — consumes the **searched** repair via `discovered_repair()`), and
  `entelechy study --searched` (the repair loop scored by execution — discovers
  reply/verify L3, knowledge/retrieval L2, decomposition/resolve_all L5).

The CLI surface: `init, objective, capability, eval, design, study [--real]
[--searched] [--out], trace, replay, counterfactual, diff, assure, release, serve,
committee, audit, demo, ops, drill [--kind restore|rollback|audit|slo|drift|degraded|all],
quickstart [--out], requirements [--check [--phase pN]]` (+ `infer` under
`--features openai`).
