//! Signing keys with rotation and revocation (PRD 17.2, AU-2, 5.9 signature
//! agility).
//!
//! Signatures record a key identifier so keys can be rotated and revoked (PRD
//! 5.9). Revoked keys are never selected for new writes, signatures or encryption
//! (PRD 17.2, AU-2).
//!
//! Two interchangeable signature primitives sit behind the same [`KeyRing`]
//! interface (PRD 5.9 signature agility):
//!
//! - **Default build:** a non-cryptographic MAC placeholder (FNV-1a over key +
//!   payload). It exercises the key-id / rotation / revocation interface without
//!   pulling in a crypto dependency, keeping the default build dependency-light.
//!   It is *not* unforgeable and must not be relied on for integrity.
//! - **`ed25519` feature:** real Ed25519 asymmetric signatures (RFC 8032). Each
//!   stored key string is used as deterministic seed material (SHA-256 → 32-byte
//!   seed → keypair), so signing needs no RNG and the same payload always yields
//!   the same signature. The signature is `ed25519:<hex of 64 bytes>` and
//!   verification checks it with `verify_strict` against the key's public half.
//!
//! The primitive is chosen at compile time; the `KeyRing` type, its methods and
//! every caller are identical either way. A signature string is only ever
//! produced and checked within one build, so the two encodings never need to
//! interoperate.

use std::collections::{BTreeMap, BTreeSet};

/// A ring of signing keys keyed by key id, with a revocation list (PRD 17.2).
#[derive(Clone, Debug, Default)]
pub struct KeyRing {
    secrets: BTreeMap<String, String>,
    revoked: BTreeSet<String>,
}

impl KeyRing {
    /// Create an empty ring.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or rotate in a key. Under the `ed25519` feature the string is used as
    /// deterministic seed material for the key's Ed25519 keypair (PRD 5.9).
    pub fn add_key(&mut self, key_id: impl Into<String>, secret: impl Into<String>) {
        self.secrets.insert(key_id.into(), secret.into());
    }

    /// Revoke a key. It can no longer sign, but existing signatures can still be
    /// checked for provenance (they will report as revoked at validation time).
    pub fn revoke(&mut self, key_id: &str) {
        self.revoked.insert(key_id.to_string());
    }

    /// Whether a key is revoked.
    pub fn is_revoked(&self, key_id: &str) -> bool {
        self.revoked.contains(key_id)
    }

    /// Sign a payload with a key. Returns `None` if the key is unknown or revoked
    /// — revoked keys are never used for new signatures (AU-2).
    pub fn sign(&self, key_id: &str, payload: &[u8]) -> Option<String> {
        if self.revoked.contains(key_id) {
            return None;
        }
        let secret = self.secrets.get(key_id)?;
        Some(sign_payload(secret, payload))
    }

    /// Verify a signature. Verification succeeds on signature match regardless of
    /// revocation; callers consult [`KeyRing::is_revoked`] separately so a revoked
    /// key's old approvals fail closed at validation (PRD 17.3 revocation survives).
    pub fn verify(&self, key_id: &str, payload: &[u8], signature: &str) -> bool {
        match self.secrets.get(key_id) {
            Some(secret) => verify_payload(secret, payload, signature),
            None => false,
        }
    }
}

// --- Signature primitive: non-cryptographic MAC placeholder (default build) ---

#[cfg(not(feature = "ed25519"))]
fn sign_payload(secret: &str, payload: &[u8]) -> String {
    fnv_mac(secret, payload)
}

#[cfg(not(feature = "ed25519"))]
fn verify_payload(secret: &str, payload: &[u8], signature: &str) -> bool {
    fnv_mac(secret, payload) == signature
}

/// FNV-1a over the key and payload — a cheap, deterministic placeholder. Not a
/// cryptographic MAC: it is forgeable and must not be relied on for integrity.
#[cfg(not(feature = "ed25519"))]
fn fnv_mac(secret: &str, payload: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in secret
        .bytes()
        .chain(std::iter::once(0u8))
        .chain(payload.iter().copied())
    {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    format!("mac1:{h:016x}")
}

// --- Signature primitive: real Ed25519 asymmetric signatures (`ed25519`) ---

/// Derive a deterministic Ed25519 signing key from a key string. The string is
/// hashed to a fixed 32-byte seed (SHA-256), so any key string is accepted and
/// the same string always yields the same keypair — no RNG, no stored keypair.
#[cfg(feature = "ed25519")]
fn ed25519_signing_key(secret: &str) -> ed25519_dalek::SigningKey {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(secret.as_bytes());
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&digest);
    ed25519_dalek::SigningKey::from_bytes(&seed)
}

#[cfg(feature = "ed25519")]
fn sign_payload(secret: &str, payload: &[u8]) -> String {
    use ed25519_dalek::Signer;
    let signature = ed25519_signing_key(secret).sign(payload);
    format!("ed25519:{}", hex::encode(signature.to_bytes()))
}

#[cfg(feature = "ed25519")]
fn verify_payload(secret: &str, payload: &[u8], signature: &str) -> bool {
    let Some(hex_sig) = signature.strip_prefix("ed25519:") else {
        return false;
    };
    let Ok(raw) = hex::decode(hex_sig) else {
        return false;
    };
    let Ok(bytes) = <[u8; 64]>::try_from(raw) else {
        return false;
    };
    let sig = ed25519_dalek::Signature::from_bytes(&bytes);
    // `verify_strict` rejects signatures made under non-canonical / small-order
    // public keys, hardening verification against adversarial inputs.
    ed25519_signing_key(secret)
        .verifying_key()
        .verify_strict(payload, &sig)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let mut ring = KeyRing::new();
        ring.add_key("k1", "secret");
        let sig = ring.sign("k1", b"hello").unwrap();
        assert!(ring.verify("k1", b"hello", &sig));
        assert!(!ring.verify("k1", b"tampered", &sig));
    }

    #[test]
    fn revoked_key_cannot_sign() {
        let mut ring = KeyRing::new();
        ring.add_key("k1", "secret");
        ring.revoke("k1");
        assert!(ring.sign("k1", b"x").is_none());
        assert!(ring.is_revoked("k1"));
    }

    #[test]
    fn wrong_key_does_not_verify() {
        let mut ring = KeyRing::new();
        ring.add_key("k1", "secret-one");
        ring.add_key("k2", "secret-two");
        let sig = ring.sign("k1", b"payload").unwrap();
        // A signature made under k1 must not verify under a different key.
        assert!(!ring.verify("k2", b"payload", &sig));
        // An unknown key never verifies.
        assert!(!ring.verify("missing", b"payload", &sig));
    }

    #[cfg(feature = "ed25519")]
    mod ed25519 {
        use super::*;

        #[test]
        fn signature_is_real_ed25519() {
            let mut ring = KeyRing::new();
            ring.add_key("k1", "seed-material");
            let sig = ring.sign("k1", b"hello").unwrap();
            // Real Ed25519: tagged and 64 bytes on the wire, not the FNV placeholder.
            let hex_sig = sig.strip_prefix("ed25519:").expect("ed25519 tag");
            assert_eq!(hex::decode(hex_sig).unwrap().len(), 64);
            assert!(!sig.starts_with("mac1:"));
        }

        #[test]
        fn signing_is_deterministic() {
            // RFC 8032 Ed25519 signatures are deterministic: same key + payload
            // => identical signature, with no RNG involved.
            let mut ring = KeyRing::new();
            ring.add_key("k1", "seed-material");
            assert_eq!(ring.sign("k1", b"abc"), ring.sign("k1", b"abc"));
            assert_ne!(ring.sign("k1", b"abc"), ring.sign("k1", b"abd"));
        }

        #[test]
        fn distinct_keys_produce_distinct_signatures() {
            let mut ring = KeyRing::new();
            ring.add_key("k1", "seed-one");
            ring.add_key("k2", "seed-two");
            assert_ne!(ring.sign("k1", b"msg"), ring.sign("k2", b"msg"));
        }

        #[test]
        fn malformed_signatures_fail_closed() {
            let mut ring = KeyRing::new();
            ring.add_key("k1", "seed-material");
            // Missing tag, bad hex, wrong length, and the placeholder encoding all
            // fail verification rather than erroring.
            assert!(!ring.verify("k1", b"hello", "deadbeef"));
            assert!(!ring.verify("k1", b"hello", "ed25519:zz"));
            assert!(!ring.verify("k1", b"hello", "ed25519:00"));
            assert!(!ring.verify("k1", b"hello", "mac1:0000000000000000"));
        }
    }
}
