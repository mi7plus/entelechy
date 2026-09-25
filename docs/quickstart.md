# Quickstart

Goal: from a clean clone to running the Phase 0 loop and a real model in well
under 30 minutes (PRD DX-1, Q7).

## 1. Build and test (~2–5 min)

```bash
git clone https://github.com/mi7plus/entelechy && cd entelechy
cargo test --workspace
```

The toolchain is pinned by `rust-toolchain.toml`. The default build pulls only
lightweight dependencies.

## 2. Run the Phase 0 slice

```bash
# Execute -> journal -> deterministic replay + crash-injection effect safety
cargo run -p entelechy-cli -- demo --out ./entelechy-demo

# The full improve loop on the simulated helpdesk:
#   baseline -> signed StudyPlan -> structural safety -> diagnose -> repair
#   -> rolling validation -> holdout gate
cargo run -p entelechy-cli -- study
```

Other commands map to the lifecycle (PRD §16.1):

```bash
cargo run -p entelechy-cli -- objective     # interview -> GoalSpec -> EvalContract
cargo run -p entelechy-cli -- capability    # CapabilityGraph, CD-7 attestation
cargo run -p entelechy-cli -- eval          # contract, power check, holdout gate
cargo run -p entelechy-cli -- assure        # SafetyCase
cargo run -p entelechy-cli -- release       # promote L0->L2 through the gate
cargo run -p entelechy-cli -- diff          # structural IR diff
cargo run -p entelechy-cli -- requirements  # the §17.6 requirement registry
```

## 3. Use a self-hosted open-source model

Any OpenAI-compatible server works — Ollama, vLLM, llama.cpp, LM Studio, LocalAI.

```bash
# Ollama:
ollama serve & ollama pull llama3.1
cargo run -p entelechy-cli --features openai -- infer --model llama3.1

# vLLM / llama.cpp / LM Studio: point --base-url at the server, e.g.
cargo run -p entelechy-cli --features openai -- \
  infer --base-url http://localhost:8000/v1 --model my-model --api-key "$TOKEN"

# Hosted https provider (adds a TLS transport):
cargo run -p entelechy-cli --features openai-tls -- \
  infer --base-url https://api.example.com/v1 --model gpt-x --api-key "$TOKEN"
```

## 4. Serve the local HTTP API (optional)

```bash
cargo run -p entelechy-cli -- serve --port 8787
curl -s -XPOST http://127.0.0.1:8787/v1/status \
  -H 'Authorization: Bearer local-dev-token' -H 'X-Protocol-Version: 1.0' -d '{}'
```

## Next steps

- Read [concepts.md](concepts.md) for the model, then
  [ARCHITECTURE.md](../ARCHITECTURE.md) for the crate map.
- [phase0.md](phase0.md) explains the Phase 0 milestone and exit criteria.
