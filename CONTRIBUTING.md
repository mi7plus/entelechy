# Contributing to Entelechy

Thanks for your interest. Entelechy is open source under the MIT licence (PRD
goal G11). This guide covers how to propose changes and what the CI gates expect.

## Ground rules

- Be respectful; see the [Code of Conduct](CODE_OF_CONDUCT.md).
- By contributing you agree your work is licensed under MIT and you sign off each
  commit under the **Developer Certificate of Origin** (DCO, Q26).

### DCO sign-off

Every commit must carry a `Signed-off-by` line matching the author:

```
git commit -s -m "your message"
```

## Development

Prerequisites: a recent stable Rust toolchain (pinned in `rust-toolchain.toml`).

```bash
cargo build --workspace
cargo test --workspace
```

Optional heavy backends are behind features (kept off by default so the core
build stays lean):

```bash
cargo test -p entelechy-gateway --features wasm         # wasmtime sandbox
cargo test -p entelechy-gateway --features openai       # model gateway (self-hosted)
cargo test -p entelechy-gateway --features openai-tls   # + hosted https providers
```

## What CI checks (run these before pushing)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p entelechy-gateway --all-targets --features wasm,openai-tls -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps        # docs must build (no broken intra-doc links)
```

Licences of dependencies are audited with `cargo deny check` against `deny.toml`
(all dependencies must be permissive and MIT-compatible — G11).

## Conventions

- **Every crate cites its PRD section(s)** in its crate-level `//!` docs; new code
  should reference the requirement IDs it satisfies (OC-*, IR-I*, EV-*, etc.), per
  the traceability rule (§17.5). A requirement is not "implemented" just because
  code exists — link a verifying test/proof.
- Keep the default build dependency-light; put heavy or optional dependencies
  behind a Cargo feature.
- `#![forbid(unsafe_code)]` is workspace policy.
- Public items are documented; prefer small, focused modules.

## Pull requests

- Reference the requirement IDs and any issue the PR addresses (§17.5).
- Keep changes scoped; add tests for new behavior.
- CI must be green (both OSes, all feature backends, fmt, clippy, docs, licences).

## Governance

See [MAINTAINERS](MAINTAINERS) for the decision and release process (Q26).
Security issues follow [SECURITY.md](SECURITY.md), not public issues.
