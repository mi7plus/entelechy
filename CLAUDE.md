# Entelechy — engineering invariants

Durable rules for every session in this repo. Not a progress log (see
`NEXT_SESSION_PROMPT.md` and `CHANGELOG.md` for state).

## Build & verify
- This machine OOM-kills a cold full build (`exit 137`). Cap parallelism:
  `cargo <cmd> --workspace -j 2`.
- The full gate before declaring a change done (all must be clean):
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`
- Feature-gated engines: `wasm` (wasmtime sandbox), `openai` / `openai-tls`
  (model gateway). Also clippy the gateway features:
  `cargo clippy -p entelechy-gateway --all-targets --features wasm,openai-tls -- -D warnings`.

## Workspace invariants
- Every crate is `#![forbid(unsafe_code)]`; workspace lints set
  `unsafe_code = "forbid"` and `missing_docs = "warn"` — keep all public items
  documented (rustdoc `-D warnings` enforces it).
- Each crate's `//!` doc cites its PRD section(s). Commit messages reference the
  PRD requirement IDs they satisfy/change/defer (OC-*, IR-I*, EV-*, MK-*, CD-*,
  RK-*, AC-*, PV-*, RS-*, EL-*, RB-*, PC-*, DR-*, OL-*, AU-*, SL-*, TR-*, DX-*).
- Keep the default build dependency-light; heavy deps go behind features.

## Git / tooling
- Commit and push to `main` only when the user asks (handoff docs are the
  exception). Branch first if asked to work off `main` for a PR.
- `gh` is NOT authenticated. Push over the SSH remote; for GitHub reads use
  unauthenticated `curl https://api.github.com/repos/mi7plus/entelechy/...`.
- Windows + git-bash. Stray 0-byte files have slipped into commits from shell
  quirks before — run `git status` and inspect before every commit.

## PRD source
- The spec is `Entelechy_PRD_v11_FINAL.docx` in `~/Downloads` (outside the repo).
  The §21 roadmap defines phases 0–5; §17.6 is the cross-cutting requirement
  registry with each requirement's first-enforced phase.
