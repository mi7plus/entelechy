# Entelechy

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

# Compile the EvalContract, check power, exercise the holdout gate + firewall
cargo run -p entelechy-cli -- eval

# Run the assurance compiler (SafetyCase) and drive a maturity promotion
cargo run -p entelechy-cli -- assure
cargo run -p entelechy-cli -- release

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

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — crate map and what is implemented vs. scaffolded.
- [`docs/phase0.md`](docs/phase0.md) — the Phase 0 milestone and how this code maps to it.
- `crates/` — the 24-crate workspace from PRD §24.

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
