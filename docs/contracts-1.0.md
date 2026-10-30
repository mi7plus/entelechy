# Entelechy 1.0 contract freeze

This document is the normative record of the contracts Entelechy freezes at the
**1.0 boundary** — the Phase-1 exit (PRD §21). Freezing means: the externally
observable byte formats and their version numbers are a compatibility promise,
and stored artifacts, recorded journals and plugin manifests stay readable across
later releases. It discharges the Phase-1 "1.0 contract freeze" gate and the
cross-cutting requirements **AC-1/AC-2** (schema id + version, fail-closed reads),
**PC-1/PC-2/PC-3** (protocol/plugin negotiation) and **RK-2/RS-1** (journal +
terminal-state replay).

The machine-readable twin of this document is
`entelechy_contracts::FROZEN_CONTRACTS` (crate `entelechy-contracts`, module
`freeze`). The two are kept in lock-step by
`entelechy-cli/tests/contract_freeze.rs`.

## Compatibility policy

- **Versioning.** Each frozen contract carries an explicit version constant in its
  owning crate. Semantic schemas (the Design IR) use a `major.minor.patch`
  `SchemaVersion`; byte/wire formats use a monotone integer; the protocol uses
  `major.minor`. A **breaking** change (a removed/renamed field, a changed tag, a
  changed byte encoding, a narrowed acceptance) bumps the major (or the single
  integer). A backward-compatible **addition** bumps the minor (serde `#[serde(default)]`
  keeps older payloads readable).
- **Fail closed.** Readers declare the versions they accept and refuse anything
  outside the window with a typed error rather than guessing (AC-1; protocol
  `NegotiationError`/`LoadError`, PC-1/PC-3). Negotiation never silently drops a
  security-relevant guarantee (PC-2).
- **Identity stability.** The canonical serialization is byte-stable within a
  canonical version, so every `ArtifactId` ever computed stays valid (AC-2). The
  artifact header participates in the content hash, so a schema/version change
  yields a different id by construction.
- **A frozen version only moves as a deliberate, documented event:** bump the
  constant, update the golden value, update the row here **and** in
  `FROZEN_CONTRACTS`, and add a CHANGELOG entry. The gates below make skipping any
  of these steps fail CI.

## The frozen contracts

| Schema id | Owner crate | Version | Constant | Byte/behaviour gate | Requirements |
|---|---|---|---|---|---|
| `entelechy.canonical-serialization` | `entelechy-artifacts` | `1` | `CANONICAL_SERIALIZATION_VERSION` | `tests/golden_format.rs` | AC-1, AC-2 |
| `entelechy.design` | `entelechy-ir` | `0.1.0` | `DESIGN_SCHEMA_VERSION` | `Program` serde roundtrip + `validate` | AC-1, IR-I8 |
| `entelechy.journal` | `entelechy-runtime` | `1` | `JOURNAL_FORMAT_VERSION` | `tests/golden_journal.rs` | RK-2, RS-1 |
| `entelechy.model-request` | `entelechy-gateway` | `1` | `NATIVE_REQUEST_SCHEMA_VERSION` | `golden_journal.rs` (captured identity) | PV-1 |
| `entelechy.failure-ontology` | `entelechy-failure` | `1` | `ONTOLOGY_VERSION` | ontology tests | PRD 10.1, 22 |
| `entelechy.protocol` | `entelechy-protocol` | `1.0` | `CURRENT_PROTOCOL_VERSION` | negotiation tests | PC-1, PC-2, PC-3 |

### `entelechy.canonical-serialization` (PRD §5.9, §17, Q22)

The byte-level canonical JSON encoding and the SHA-256 content address derived from
it. `golden_format.rs` pins both the exact bytes of a fixture and its digest, and
asserts `CANONICAL_SERIALIZATION_VERSION == 1`. The per-artifact self-describing
header (`ArtifactHeader`: `schema_id`, `schema_version: SchemaVersion`,
`producer_version`, `canonical_version`, `tenant`) is part of this contract: it is
hashed into every id, which is what makes fail-closed reads safe. Floats are
excluded from canonical payloads (documented AC-2 limitation).

### `entelechy.design` (PRD §5.2, §5.4, §7)

The typed IR that is a `Design` artifact payload: `Program { authority, root }`,
the `Node`/`NodeKind` tree (`Llm`, `Tool`, `Code`, `Seq`, `Par`, `Map`, `Branch`,
`Loop`, `Delegate`, `Verify`, `Mem`, `Gate`, `Human`), the `Condition` language,
and the **task/checker node interfaces** — `ToolNode`/`CodeNode` (tasks) and
`VerifyNode` (checker), plus the in-process `CodeFn`/`CheckerFn` registration
signatures in `entelechy-runtime`. `AuthorityEnvelope` (PRD §5.4) is part of this
schema: designs may not widen it and `Delegate` only narrows it (A1–A3 / IR-I6).
`NodeKind`/`Condition` are externally tagged to keep the recursive IR
serializable, human-readable and diffable (IR-I8). Frozen at `0.1.0` because the
IR still gains nodes pre-maturity; additions are minor bumps guarded by
`#[serde(default)]`.

### `entelechy.journal` (PRD §5.2, §8.2, §8.4–8.6, RK-2)

The append-only execution journal: `Journal { run_id, entries }`, `JournalEntry {
seq, node_path, event }`, and the `JournalEvent` tag set (`model-call`,
`tool-effect`, `policy-decision`). A recorded journal must stay replayable, so the
field names, the `kind` tags and the entry structure are frozen within
`JOURNAL_FORMAT_VERSION`. `golden_journal.rs` pins the exact canonical bytes of a
deterministic run and asserts the version constant alongside it. The version is a
module constant, not a serialized field, so the wire format is unchanged.

### `entelechy.model-request` (PRD §5.10, PV-1)

The native `ModelRequest` shape and the `ProviderIdentity` captured for every model
call (`provider`, `advertised_model`, `endpoint`, `request_schema_version`,
`sampling`, `revision`). `NATIVE_REQUEST_SCHEMA_VERSION` names the native request
schema; external providers record their own string (the OpenAI gateway records
`openai.chat.v1`). The captured identity is exercised by the journal golden.

### `entelechy.failure-ontology` (PRD §10.1, §22)

The failure `Family`/`FailureClass` vocabulary, versioned by `ONTOLOGY_VERSION` so
a diagnosis stored under one ontology keeps its meaning. Locked early per the
PRD §22 decision.

### `entelechy.protocol` (PRD §16.6, Q29)

The API/protocol/plugin compatibility layer: `ProtocolVersion`/`VersionRange`, the
`negotiate`/`negotiate_secure` semantics (highest common version, fail-closed on no
overlap — PC-1; no silent security downgrade — PC-2), `PluginManifest::check_load`
(fails closed — PC-3), and the `Deprecation` window (Q29). `CURRENT_PROTOCOL_VERSION`
is the version this build speaks.

## How the gates compose

Three layers together make "change a frozen format without a version bump" fail CI:

1. **Golden byte tests** (`golden_format.rs`, `golden_journal.rs`) trip on any
   change to the serialized bytes or the content-address digest. The failure
   message points here and to the version-bump procedure.
2. **Version-pin asserts** sit next to the byte fixtures (`..._version_is_pinned`),
   so a format change and the version bump it requires are reviewed in the same
   diff.
3. **The freeze registry + consistency gate.** `FROZEN_CONTRACTS` restates every
   frozen version; `contract_freeze.rs` asserts each restatement equals the
   authoritative constant in the owning crate, and that no frozen contract is added
   without a binding. Bumping a constant therefore forces a visible edit to the
   frozen set — the deliberate compatibility event this freeze is about.

No test can prove a byte change is *semantically* breaking, so the version bump
itself is a human judgement; the gates guarantee it is a **visible, reviewed** one
rather than a silent drift.

## Version-bump procedure

When a frozen format or its acceptance changes on purpose:

1. Bump the owning version constant (major/integer for breaking, minor for additive).
2. Update the pinned golden value(s) in the owning crate's `tests/`.
3. Update the contract's row in this document **and** in
   `entelechy_contracts::FROZEN_CONTRACTS`.
4. Add a CHANGELOG entry under the compatibility section, citing the requirement IDs.
5. For additive IR/journal changes, keep older payloads readable (`#[serde(default)]`)
   so the minor bump is honest.

## Not frozen (still provisional pre-maturity)

Internal crate APIs, CLI output text, the in-memory store layout, and engines behind
feature flags (`wasm`, `openai`, `openai-tls`) are **not** part of the 1.0 freeze and
may change without a schema bump. Only the contracts in the table above carry the
compatibility promise.
