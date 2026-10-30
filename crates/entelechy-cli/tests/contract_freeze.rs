//! Cross-crate consistency gate for the 1.0 contract freeze (PRD §21 Phase-1 exit).
//!
//! The frozen-contract registry in `entelechy-contracts` restates each schema's
//! frozen version. This test binds those restatements to the authoritative
//! constants in the owning crates, so bumping a constant without updating the
//! frozen set (or vice versa) fails CI. The per-format golden tests cover the
//! byte encoding; this covers the version numbers and the registry's coverage.
//!
//! If this fails, reconcile `docs/contracts-1.0.md`,
//! `entelechy_contracts::FROZEN_CONTRACTS` and the owning constant — a frozen
//! version only moves as a deliberate, documented compatibility event.

use entelechy_contracts::{freeze, FROZEN_CONTRACTS};

fn frozen_version(schema_id: &str) -> &'static str {
    freeze::find(schema_id)
        .unwrap_or_else(|| panic!("frozen registry is missing '{schema_id}'"))
        .version
}

/// Assert the registry's frozen version for `schema_id` equals the version its
/// owning constant renders (`owning` is that rendering, computed at the call site
/// because the owning constants are of heterogeneous types).
fn assert_frozen(schema_id: &str, owning: String) {
    assert_eq!(
        frozen_version(schema_id),
        owning.as_str(),
        "frozen '{schema_id}' version disagrees with its owning constant",
    );
}

#[test]
fn frozen_versions_match_owning_constants() {
    assert_frozen(
        "entelechy.canonical-serialization",
        entelechy_artifacts::CANONICAL_SERIALIZATION_VERSION.to_string(),
    );
    assert_frozen(
        "entelechy.design",
        entelechy_ir::DESIGN_SCHEMA_VERSION.to_string(),
    );
    assert_frozen(
        "entelechy.journal",
        entelechy_runtime::JOURNAL_FORMAT_VERSION.to_string(),
    );
    assert_frozen(
        "entelechy.model-request",
        entelechy_gateway::NATIVE_REQUEST_SCHEMA_VERSION.to_string(),
    );
    assert_frozen(
        "entelechy.failure-ontology",
        entelechy_failure::ONTOLOGY_VERSION.to_string(),
    );
    assert_frozen(
        "entelechy.protocol",
        entelechy_protocol::CURRENT_PROTOCOL_VERSION.to_string(),
    );
}

#[test]
fn design_schema_id_matches_owning_constant() {
    assert_eq!(
        freeze::find("entelechy.design").unwrap().schema_id,
        entelechy_ir::DESIGN_SCHEMA_ID,
    );
}

#[test]
fn every_frozen_contract_is_bound_here() {
    // Guard against a new frozen contract being added to the registry without a
    // matching version binding above. If this fails, add the new schema to
    // `frozen_versions_match_owning_constants` and to docs/contracts-1.0.md.
    let bound = [
        "entelechy.canonical-serialization",
        "entelechy.design",
        "entelechy.journal",
        "entelechy.model-request",
        "entelechy.failure-ontology",
        "entelechy.protocol",
    ];
    for c in FROZEN_CONTRACTS {
        assert!(
            bound.contains(&c.schema_id),
            "frozen contract '{}' has no version binding in contract_freeze.rs",
            c.schema_id
        );
    }
    assert_eq!(bound.len(), FROZEN_CONTRACTS.len());
}
