//! Cross-crate integration tests for the Entelechy pipeline.
//!
//! This crate ships no library code; its purpose is the `tests/` directory, which
//! exercises whole-pipeline contracts that no single crate's unit tests can see:
//! design synthesis → IR validation → runtime execution → deterministic replay
//! (PRD 3.1, 8.2), and the content-addressing identity contract across crates
//! (PRD 5.9). Keeping these in a dedicated member crate means they run under the
//! ordinary `cargo test --workspace` gate.
#![forbid(unsafe_code)]
