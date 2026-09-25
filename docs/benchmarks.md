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
| `to_canonical_bytes` (32-node doc) | 39.7 µs | 39.3–40.2 µs |
| `ArtifactId::of` (32-node doc)     | 123.0 µs | 121.0–125.1 µs |

### `entelechy-runtime` — interpreter hot path

| Benchmark | Median | 95% range |
|---|---|---|
| `execute` (Seq: LLM→code→verify)   | 6.31 µs | 6.28–6.36 µs |
| `replay` (same program vs journal) | 2.76 µs | 2.67–2.85 µs |

## Notes

- **Hashing dominates id computation.** `ArtifactId::of` (~123 µs) is ~3× the
  canonical serializer alone (~40 µs); the difference is the `serde_json::to_value`
  projection plus SHA-256 over the document. The JCS serializer is not the cost
  center — if id computation ever shows up in a search profile, hash the canonical
  bytes directly rather than round-tripping through `Value`.
- **Replay is ~2.3× lighter than execute** (2.76 µs vs 6.31 µs), confirming the
  recorded-effect path (PRD 8.2) avoids the live model dispatch.
- **Interpreter overhead is single-digit µs**, so per-run study cost is dominated
  by real model/tool latency, not the kernel.

## Environment

Recorded 2026-09-25 on a Windows 11 developer machine (background-noisy; the
artifacts benchmarks showed up to 20% sample outliers). Treat these as ballpark
figures for relative comparison, not absolute performance guarantees. Re-baseline
on the target hardware before drawing conclusions about absolute throughput.
