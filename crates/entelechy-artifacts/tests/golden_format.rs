//! Golden format-stability tests for the content-addressing contract (PRD 5.9).
//!
//! `CANONICAL_SERIALIZATION_VERSION` is a compatibility promise: the byte-level
//! canonical encoding — and therefore every `ArtifactId` ever computed — must not
//! change without a deliberate version bump and a CHANGELOG entry. These tests pin
//! the exact canonical bytes and the exact SHA-256 digest for a fixed fixture, so
//! an accidental change to the serializer fails CI instead of silently
//! invalidating stored ids and lineage.
//!
//! If one of these fails because the format was changed *on purpose*, bump
//! `CANONICAL_SERIALIZATION_VERSION`, update the golden value here, and note it in
//! CHANGELOG.md under the format-compatibility section.

use entelechy_artifacts::{to_canonical_bytes, ArtifactId, CANONICAL_SERIALIZATION_VERSION};
use serde_json::json;

/// A fixed fixture exercising key sorting, nesting, arrays, and the JSON scalar
/// types Entelechy artifacts use (integers/strings/bools/null — floats are
/// excluded per the canonical module's documented AC-2 limitation).
fn fixture() -> serde_json::Value {
    json!({
        "b": "second",
        "a": 1,
        "nested": { "y": true, "x": [1, 2, 3] },
        "z": null
    })
}

#[test]
fn canonical_serialization_version_is_pinned() {
    // A change here is a deliberate, reviewed compatibility event.
    assert_eq!(CANONICAL_SERIALIZATION_VERSION, 1);
}

#[test]
fn canonical_bytes_are_stable() {
    let bytes = to_canonical_bytes(&fixture());
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(
        text,
        r#"{"a":1,"b":"second","nested":{"x":[1,2,3],"y":true},"z":null}"#
    );
}

#[test]
fn artifact_id_digest_is_stable() {
    let id = ArtifactId::of(&fixture()).unwrap();
    assert_eq!(
        id.to_string(),
        "sha256:21d2849a1e396079a563fbaf947809d13c9d64dd0252ec86fec67687be5d9651",
        "content-addressing digest changed; if intentional, bump \
         CANONICAL_SERIALIZATION_VERSION and update this golden value"
    );
}
