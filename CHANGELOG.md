# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
Semantic Versioning with explicit compatibility promises for the IR, artifact
schemas and journal format (PRD §5.9, §16.5, §16.6).

## [Unreleased]

Pre-1.0. Interfaces are still provisional until the 1.0 contract freeze (PRD §21,
Phase 1). This section tracks the initial implementation of the PRD v11 baseline.

### Added
- Workspace implementing the PRD v11 architecture (§24), plus a dedicated
  `entelechy-integration` crate for cross-crate pipeline tests.
- Property tests (`proptest`) for the content-addressing trust root, criterion
  benchmarks for the canonicalization and interpreter hot paths, runnable
  doctests, and `cargo fuzz` targets for canonical JSON and IR deserialization.
- CI: an MSRV (1.85) build job and a non-blocking coverage job; the RUSTSEC
  advisory scan now runs on push/schedule only (not on PRs).
- A pull-request template surfacing the CI gates and DCO sign-off.
- `missing_docs = "warn"` as a workspace lint, so the (already universal) public-API
  documentation discipline is enforced and the rustdoc `-D warnings` gate fails on a
  regression.
- Golden format-stability tests that pin the canonical-JSON bytes, the SHA-256
  `ArtifactId` digest, and the journal wire format for fixed fixtures — turning the
  §5.9 / §16.5–16.6 compatibility promises into enforced tests (an accidental
  format change fails CI; a deliberate one requires bumping the serialization
  version and updating the golden value).

### Fixed
- HTTP server hardening (loopback DoS): the request body is now capped
  (`413` above 1 MiB) instead of allocating `Content-Length` verbatim, header
  count is bounded (`431`), and per-connection read/write timeouts prevent a
  single stalled client from wedging the serial accept loop.
- OpenAI gateway: the client now bounds the TCP dial with `connect_timeout`, so a
  wrong `--base-url` fails within the timeout instead of blocking on the OS default.
- Bootstrap PRNG: guard the one seed that would zero the xorshift state (which would
  collapse the confidence interval); such a seed now falls back to a non-zero state.
- IR with node model, value envelope (taint + confidentiality), effect taxonomy,
  AuthorityEnvelope and static compiler invariants (§7, §5.4, §5.6).
- Content-addressed artifacts (RFC 8785 + SHA-256, digest agility), schema
  versioning, tenant-scoped stores, deployment profiles, erasure ledger and an
  append-only metadata store (§5.9, §17).
- Runtime: interpreter, append-only journal, deterministic replay, counterfactual
  replay, typed terminal states, memory tiers, cluster work queue + worker pool,
  and a compiled backend with an interpreter-vs-codegen conformance/Q10 gate
  (§8, §10.2, §12, §17, Q10).
- Evaluation subsystem (contract, power analysis, holdout gate + firewall, stats,
  retrieval metrics, adversarial generation, judge calibration) (§9, MK-4).
- Policy engine (native core + Cedar-style adapter), identity/approvals with a
  tamper-evident audit log, gateways (model/tool + WASM sandbox), capability
  discovery + tool-synthesis governance, failure analyzer, design/search,
  assurance + release ladder, objective compiler, benchmark governance + task
  synthesis, protocol negotiation, requirement traceability, and a local HTTP API.
- OpenAI-compatible model gateway for self-hosted OSS models (Ollama/vLLM/
  llama.cpp/LM Studio), with an optional TLS transport for hosted providers (Q7).
- CI (build/test on Linux + Windows, feature matrix, fmt, clippy, `cargo deny`
  licence audit) with SHA-pinned actions and Dependabot; project-health docs.

[Unreleased]: https://github.com/mi7plus/entelechy/commits/main
