# Benchmark baseline

Regression baselines for the two hot paths that get multiplied across a study:
content addressing (every artifact write and id comparison) and the interpreter
(every candidate a search evaluates). Reproduce with:

```bash
cargo bench -p entelechy-artifacts
cargo bench -p entelechy-runtime
```

Criterion stores raw measurements under `target/criterion/` (gitignored); a
subsequent `cargo bench` reports deltas against them. The numbers below are a
human-readable snapshot to compare against, not a gate.

## Results

Median of 100 samples, release build (`lto = "thin"`).

### `entelechy-artifacts` — content-addressing hot path

| Benchmark | Median | 95% range |
|---|---|---|
| `to_canonical_bytes` (32-node doc) | 40.1 µs | 39.6–40.5 µs |
| `ArtifactId::of` (32-node doc)     | 115.2 µs | 112.6–117.8 µs |

### `entelechy-runtime` — interpreter hot path

| Benchmark | Median | 95% range |
|---|---|---|
| `execute` (Seq: LLM→code→verify)   | 6.38 µs | 6.33–6.44 µs |
| `replay` (same program vs journal) | 2.90 µs | 2.86–2.94 µs |

## Notes

- **Hashing dominates id computation.** `ArtifactId::of` (~115 µs) is ~3× the
  canonical serializer alone (~40 µs); the difference is the `serde_json::to_value`
  projection plus SHA-256 over the document. The JCS serializer is not the cost
  center — if id computation ever shows up in a search profile, hash the canonical
  bytes directly rather than round-tripping through `Value`.
- **Replay is ~2.2× lighter than execute** (2.90 µs vs 6.38 µs), confirming the
  recorded-effect path (PRD 8.2) avoids the live model dispatch.
- **Interpreter overhead is single-digit µs**, so per-run study cost is dominated
  by real model/tool latency, not the kernel.

## Environment

Refreshed 2026-09-25 on a Windows 11 developer machine (background-noisy). Versus
the initial baseline, criterion reported `ArtifactId::of` ~5% faster and the
interpreter paths ~2% slower — all within the run-to-run noise of this machine (no
code changed the hot paths between runs), so treat these as ballpark figures for
relative comparison, not absolute performance guarantees. Re-baseline on the target
hardware before drawing conclusions about absolute throughput.
