//! Fuzz the canonical-JSON trust root (PRD 5.9, Q22).
//!
//! The canonicalizer sits under every ArtifactId, so it must never panic on any
//! input and must be a fixpoint: canonicalizing already-canonical bytes yields the
//! same bytes. Run with `cargo +nightly fuzz run canonical_roundtrip`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Only well-formed JSON reaches the canonicalizer in practice.
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        let once = entelechy_artifacts::to_canonical_bytes(&value);
        let reparsed: serde_json::Value =
            serde_json::from_slice(&once).expect("canonical output must be valid JSON");
        let twice = entelechy_artifacts::to_canonical_bytes(&reparsed);
        assert_eq!(once, twice, "canonicalization must be idempotent");
    }
});
