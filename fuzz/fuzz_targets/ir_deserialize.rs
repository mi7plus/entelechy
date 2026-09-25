//! Fuzz IR deserialization (PRD 7).
//!
//! A `Program` can arrive as untrusted JSON (e.g. a stored/deployed design), so
//! deserializing it and re-serializing the result must never panic. Run with
//! `cargo +nightly fuzz run ir_deserialize`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(program) = serde_json::from_slice::<entelechy_ir::Program>(data) {
        // A successfully-parsed program must round-trip back to JSON without panic.
        let _ = serde_json::to_vec(&program);
    }
});
