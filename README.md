# Entelechy

[![CI](https://github.com/mi7plus/entelechy/actions/workflows/ci.yml/badge.svg)](https://github.com/mi7plus/entelechy/actions/workflows/ci.yml)

An objective-driven agentic systems synthesis platform, in Rust.

Entelechy treats **architecture as a search result, not an input**. You specify
an outcome, its constraints, the authority it may exercise, and the evidence you
require; Entelechy compiles that into evaluations, synthesizes an executable
design, improves it through evidence-driven experiments, verifies it against
held-out and adversarial tests, and emits a deployable release with provenance
and a safety case.

This repository implements the specification in `Entelechy_PRD_v11_FINAL.docx`
(PRD v11). It is open source under the MIT licence (PRD goal G11).

## Status

Early implementation. The **Phase 0** core vertical slice runs end to end:
a single-agent IR is statically validated, executed while journaling every
external effect, and replayed deterministically from that journal.

```bash
# Build & test the workspace
cargo test --workspace

# Run the bundled execute -> journal -> replay demo
cargo run -p entelechy-cli -- demo --out ./entelechy-demo

# Run a full Phase 0 study loop on the simulated helpdesk
cargo run -p entelechy-cli -- study

# Run a multi-agent committee (map/reduce) — mock model, or a live one with
# --features openai and ENTELECHY_BASE_URL/ENTELECHY_MODEL set
cargo run -p entelechy-cli -- committee "How do I reset my password?"
#   size it:      --agents N (>=2; first two are analyst+responder, rest generic)
#   tune prompts: --analyst <t> --responder <t> --coordinator <t> ({input} auto-appended)

# Compile the EvalContract, check power, exercise the holdout gate + firewall
cargo run -p entelechy-cli -- eval

# Run the assurance compiler (SafetyCase) and drive a maturity promotion
cargo run -p entelechy-cli -- assure
cargo run -p entelechy-cli -- release

# Talk to a self-hosted open-source model (Ollama/vLLM/llama.cpp/LM Studio)
#   ollama serve && ollama pull llama3.1
cargo run -p entelechy-cli --features openai -- infer --model llama3.1
#   vLLM:      --base-url http://localhost:8000/v1 --api-key <token>
#   llama.cpp: --base-url http://localhost:8080/v1
#   hosted https provider (adds TLS): --features openai-tls --base-url https://... --api-key <token>

# Serve the local HTTP API (loopback only), then curl it
cargo run -p entelechy-cli -- serve --port 8787
# curl -s -XPOST http://127.0.0.1:8787/v1/status \
#   -H 'Authorization: Bearer local-dev-token' -H 'X-Protocol-Version: 1.0' -d '{}'

# Statically check a Design IR against the compiler invariants (PRD 7.4)
cargo run -p entelechy-cli -- design --validate ./entelechy-demo/design.json

# Replay a recorded journal against its design (PRD 8.2)
cargo run -p entelechy-cli -- replay \
  --design ./entelechy-demo/design.json \
  --journal ./entelechy-demo/journal.json

# List the cross-cutting requirement registry (PRD 17.6)
cargo run -p entelechy-cli -- requirements
```

## Where things are

- [`docs/quickstart.md`](docs/quickstart.md) — from clone to a running loop and a real model.
- [`docs/concepts.md`](docs/concepts.md) — the model: lifecycle, artifacts, principles.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — crate map and what is implemented vs. scaffolded.
- [`docs/phase0.md`](docs/phase0.md) — the Phase 0 milestone and how this code maps to it.
- `crates/` — the workspace crates named in PRD §24.

## Testing & benchmarks

```bash
# Whole workspace: unit + integration + doctests
cargo test --workspace

# Cross-crate pipeline tests (design -> IR -> execute -> replay, content addressing)
cargo test -p entelechy-integration

# Regression benchmarks for the content-addressing and interpreter hot paths
# (baseline numbers: docs/benchmarks.md)
cargo bench -p entelechy-artifacts
cargo bench -p entelechy-runtime

# Fuzz the canonical-JSON trust root and IR deserialization (nightly)
cargo +nightly fuzz run canonical_roundtrip   # from ./fuzz
cargo +nightly fuzz run ir_deserialize
```

## Contributing & security

- [`CONTRIBUTING.md`](CONTRIBUTING.md) — dev setup, CI gates, conventions, DCO.
- [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md), [`MAINTAINERS`](MAINTAINERS) — community & governance.
- [`SECURITY.md`](SECURITY.md) — private vulnerability reporting (coordinated disclosure).
- [`CHANGELOG.md`](CHANGELOG.md) — notable changes.

## Principles (PRD §4, abbreviated)

1. Human authority at explicit gates — the system may propose, never self-grant.
2. Hard constraints define feasibility — they are gates, not scalar penalties.
3. Evaluation is part of the specification.
4. Complexity is earned — start with the simplest viable design.
5. Every design change is a hypothesis backed by evidence.
6. Everything is an artifact with lineage.
7. Every external effect is journaled; replay uses recorded effects.
8. Least privilege by construction.

## Licence

MIT © George Olteanu
