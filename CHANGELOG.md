# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
Semantic Versioning with explicit compatibility promises for the IR, artifact
schemas and journal format (PRD §5.9, §16.5, §16.6).

## [Unreleased]

Pre-1.0. Interfaces are still provisional until the 1.0 contract freeze (PRD §21,
Phase 1). This section tracks the initial implementation of the PRD v11 baseline.

### Added
- **Multi-agent designs (Phase 1, PRD 11.4 level 4/5).** A multi-agent synthesizer
  (`synthesize_multi_agent`, a coordinator over parallel scoped `Delegate`
  sub-agents) and the level-4/5 typed edit operators `SplitParallel` (fan a single
  agent into a parallel committee) and `AddDelegate` (wrap a subgraph in a
  narrowed-authority sub-agent). Delegation narrowing is enforced at apply time
  against the authority in force at the wrap site, including nested delegates
  (A2 / IR-I6); widening is rejected. A `DesignHypothesis::decomposition`
  constructor ties these operators to the `reasoning.decomposition` failure class
  the Architecture Explorer unlocks them on. Cross-crate tests execute and replay
  the synthesized multi-agent programs end to end.
- **Study-loop operator proposal** (`entelechy_search::propose`): the search driver
  now turns a diagnosed failure class into the next design mutation, escalating
  single-agent → verification → parallel committee → multi-agent delegation only
  when the class makes the level eligible and the Architecture Explorer allows the
  unlock (within ceiling/budget, without adding authority — Q4). Level-5 delegation
  proposals build a validly-narrowed reasoning-only sub-agent authority (A2/IR-I6).
  The `study` CLI command now reports the proposed level and operators.
- **Iterative, escalating study loop.** The `study` command now runs a real
  multi-candidate search: each round it re-diagnoses the best design's remaining
  failures, proposes the next mutation, evaluates it under rolling validation, and
  adopts or escalates to the next complexity level — converging when no failure
  remains, the budget is spent, or an unlock needs approval. The demo suite now
  carries two failure classes (reply → verification/level 3, multi-part →
  decomposition/level 4), so a run visibly escalates single-agent → verification →
  parallel multi-agent committee before the holdout gate. Adds a
  `Symptom::IncompleteDecomposition` mapping to `reasoning.decomposition`.
- **Committee reduce step.** `SplitParallel` is now a full map/reduce: it produces
  `Seq[Par[specialists], coordinator]`, where a coordinator `Llm` aggregates the
  agents' outputs into one synthesized value instead of returning a raw array
  (PRD 11.4 level 4). Executes end to end with any model.
- **`entelechy study --out <dir>`** persists the run's artifacts: the final best
  design IR (`design.json`, re-validatable and diffable) and a `study-report.json`
  recording the StudyPlan commitment, baseline/best content addresses, final
  complexity level, budget usage, every hypothesis with its result, a typed
  `derived-from` lineage chain with one edge per accepted step (baseline → v1 →
  … → best, PRD 5.2), and the holdout gate outcome — a reproducible, auditable
  record (PRD principle 6, 21.1). It also writes a tamper-evident, hash-chained
  `audit-log.json` (PRD 17.2) recording the plan commitment, each hypothesis
  result, and the holdout outcome; the report commits to the audit chain head.
- **`entelechy audit <dir>`** re-verifies a persisted audit log's hash chain and
  that a study report's `audit_head` still matches the chain head, exiting
  non-zero on any tampering or mismatch (PRD 17.2). It prints the current chain
  head to record as an external anchor, and `--anchor <hash>` verifies a
  previously-held anchor is still in the chain — catching a full rewrite that an
  internal chain check alone cannot.
- **`entelechy committee` CLI command.** Builds a multi-agent committee, runs it
  against a model, and prints each sub-agent's output plus the coordinator's
  synthesized answer. Uses a live OpenAI-compatible model when built with
  `--features openai` and `ENTELECHY_BASE_URL`/`ENTELECHY_MODEL` are set; otherwise
  the deterministic mock model, so it always runs.

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
