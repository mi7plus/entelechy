//! Property tests for the content-addressing trust root (PRD 5.9, Q22).
//!
//! `ArtifactId` is the substrate for immutability and lineage, so the canonical
//! encoding must satisfy a few invariants for *every* value, not just the
//! hand-picked cases in the unit tests:
//!   * **stability** — canonicalizing already-canonical bytes is a fixpoint;
//!   * **fidelity** — canonical bytes parse back to the same semantic value;
//!   * **whitespace/formatting insignificance** — a pretty-printed value has the
//!     same canonical form (and thus the same id) as its compact form;
//!   * **determinism** — the id is a pure function of the value, and its wire
//!     string round-trips through `parse`/`Display`.
//!
//! Values are drawn from JSON with integer numbers only: the canonical module
//! documents that full RFC 8785 ECMAScript number formatting is not yet in place
//! (tracked to AC-2), so floats are intentionally excluded here.

use entelechy_artifacts::{to_canonical_bytes, ArtifactId};
use proptest::prelude::*;
use serde_json::Value;

/// A strategy for JSON values using only integer numbers (see module note).
fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::Number(n.into())),
        any::<String>().prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::hash_map(any::<String>(), inner, 0..6)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    /// Canonicalization is a fixpoint: re-canonicalizing canonical bytes is a no-op.
    #[test]
    fn canonicalization_is_idempotent(v in json_value()) {
        let once = to_canonical_bytes(&v);
        let reparsed: Value = serde_json::from_slice(&once)
            .expect("canonical bytes must be valid JSON");
        let twice = to_canonical_bytes(&reparsed);
        prop_assert_eq!(once, twice);
    }

    /// Canonical bytes preserve the value's meaning (round-trip fidelity).
    #[test]
    fn canonical_bytes_preserve_semantics(v in json_value()) {
        let bytes = to_canonical_bytes(&v);
        let back: Value = serde_json::from_slice(&bytes)
            .expect("canonical bytes must be valid JSON");
        prop_assert_eq!(back, v);
    }

    /// Whitespace and key ordering in the textual form are insignificant: a
    /// pretty-printed value hashes to the same id as its compact form.
    #[test]
    fn formatting_does_not_affect_identity(v in json_value()) {
        let compact = ArtifactId::of(&v).unwrap();
        let pretty = serde_json::to_string_pretty(&v).unwrap();
        let reparsed: Value = serde_json::from_str(&pretty).unwrap();
        let pretty_id = ArtifactId::of(&reparsed).unwrap();
        prop_assert_eq!(compact, pretty_id);
    }

    /// The id is a pure function of the value, and its wire form round-trips.
    #[test]
    fn id_is_deterministic_and_parseable(v in json_value()) {
        let a = ArtifactId::of(&v).unwrap();
        let b = ArtifactId::of(&v).unwrap();
        prop_assert_eq!(&a, &b);
        let parsed = ArtifactId::parse(&a.to_string()).unwrap();
        prop_assert_eq!(parsed, a);
    }
}
