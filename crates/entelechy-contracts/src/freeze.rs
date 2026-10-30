//! The 1.0 frozen-contract registry (PRD 5.9, 16.5, 16.6, §21 Phase-1 exit).
//!
//! Phase 1 exits by *freezing* the externally observable contracts so stored
//! artifacts, journals and plugin manifests stay readable across later releases.
//! This module is the machine-readable twin of `docs/contracts-1.0.md`: it lists
//! every frozen schema, the version it is frozen at, the constant that carries
//! that version in its owning crate, and the gate that trips if the byte format
//! changes.
//!
//! The declared versions are duplicated here on purpose. The owning crate holds
//! the authoritative constant; this registry restates it so that bumping a
//! constant forces a visible edit to the frozen set — a deliberate, reviewed
//! compatibility event — rather than a silent drift. A cross-crate test
//! (`entelechy-cli/tests/contract_freeze.rs`) asserts the two agree, and the
//! per-format golden tests assert the byte encoding itself is unchanged.

/// A contract frozen at the 1.0 boundary (PRD §21 Phase-1 exit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrozenContract {
    /// Stable schema identifier, e.g. `"entelechy.journal"`.
    pub schema_id: &'static str,
    /// The crate that owns the authoritative version constant.
    pub owner_crate: &'static str,
    /// The version this contract is frozen at, as the owning constant renders it
    /// (e.g. `"1"`, `"0.1.0"`, `"1.0"`).
    pub version: &'static str,
    /// The name of the constant in the owning crate that carries the version.
    pub version_const: &'static str,
    /// The test or check that fails if the frozen byte format changes.
    pub gate: &'static str,
    /// PRD requirement IDs the contract discharges.
    pub requirements: &'static str,
    /// The compatibility promise, in one line.
    pub promise: &'static str,
}

const fn frozen(
    schema_id: &'static str,
    owner_crate: &'static str,
    version: &'static str,
    version_const: &'static str,
    gate: &'static str,
    requirements: &'static str,
    promise: &'static str,
) -> FrozenContract {
    FrozenContract {
        schema_id,
        owner_crate,
        version,
        version_const,
        gate,
        requirements,
        promise,
    }
}

/// The contracts frozen at the 1.0 boundary.
///
/// Ordered from the lowest layer (byte encoding) up. `AuthorityEnvelope` and the
/// task/checker node interfaces are part of the Design IR and are covered by the
/// `entelechy.design` row; the per-artifact `schema_id`/`SchemaVersion` header is
/// covered by `entelechy.canonical-serialization`.
pub const FROZEN_CONTRACTS: &[FrozenContract] = &[
    frozen(
        "entelechy.canonical-serialization",
        "entelechy-artifacts",
        "1",
        "CANONICAL_SERIALIZATION_VERSION",
        "entelechy-artifacts/tests/golden_format.rs",
        "AC-1, AC-2",
        "Canonical JSON bytes and every derived artifact id are byte-stable within a canonical version.",
    ),
    frozen(
        "entelechy.design",
        "entelechy-ir",
        "0.1.0",
        "DESIGN_SCHEMA_VERSION",
        "entelechy-ir program serde roundtrip + entelechy_ir::validate",
        "AC-1, IR-I8",
        "The Design IR (nodes, task/checker nodes, AuthorityEnvelope) is diffable and migratable; a breaking shape change bumps major.",
    ),
    frozen(
        "entelechy.journal",
        "entelechy-runtime",
        "1",
        "JOURNAL_FORMAT_VERSION",
        "entelechy-runtime/tests/golden_journal.rs",
        "RK-2, RS-1",
        "Recorded journals stay replayable; the event `kind` tags and entry structure are frozen within a format version.",
    ),
    frozen(
        "entelechy.model-request",
        "entelechy-gateway",
        "1",
        "NATIVE_REQUEST_SCHEMA_VERSION",
        "entelechy-runtime/tests/golden_journal.rs (captured identity)",
        "PV-1",
        "The native ModelRequest/ProviderIdentity shape recorded for every model call is frozen within a request-schema version.",
    ),
    frozen(
        "entelechy.failure-ontology",
        "entelechy-failure",
        "1",
        "ONTOLOGY_VERSION",
        "entelechy-failure ontology tests",
        "PRD 10.1, 22",
        "Failure families and classes are a versioned vocabulary; stored diagnoses keep their meaning within an ontology version.",
    ),
    frozen(
        "entelechy.protocol",
        "entelechy-protocol",
        "1.0",
        "CURRENT_PROTOCOL_VERSION",
        "entelechy-protocol negotiation tests",
        "PC-1, PC-2, PC-3",
        "The negotiated protocol and plugin-manifest compatibility rules fail closed; a breaking change bumps the protocol major.",
    ),
];

/// Look up a frozen contract by its schema id.
pub fn find(schema_id: &str) -> Option<&'static FrozenContract> {
    FROZEN_CONTRACTS.iter().find(|c| c.schema_id == schema_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_nonempty() {
        assert!(!FROZEN_CONTRACTS.is_empty());
    }

    #[test]
    fn schema_ids_are_unique() {
        let mut ids: Vec<&str> = FROZEN_CONTRACTS.iter().map(|c| c.schema_id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate schema id in frozen registry");
    }

    #[test]
    fn every_contract_is_fully_populated() {
        for c in FROZEN_CONTRACTS {
            assert!(!c.schema_id.is_empty(), "empty schema_id");
            assert!(
                !c.owner_crate.is_empty(),
                "{}: empty owner_crate",
                c.schema_id
            );
            assert!(!c.version.is_empty(), "{}: empty version", c.schema_id);
            assert!(
                !c.version_const.is_empty(),
                "{}: empty version_const",
                c.schema_id
            );
            assert!(!c.gate.is_empty(), "{}: empty gate", c.schema_id);
            assert!(!c.promise.is_empty(), "{}: empty promise", c.schema_id);
        }
    }

    #[test]
    fn lookup_works() {
        assert_eq!(find("entelechy.journal").unwrap().version, "1");
        assert!(find("entelechy.does-not-exist").is_none());
    }
}
