//! Signing keys with rotation and revocation (PRD 17.2, AU-2, 5.9 signature
//! agility).
//!
//! Signatures record a key identifier so keys can be rotated and revoked (PRD
//! 5.9). Revoked keys are never selected for new writes, signatures or encryption
//! (PRD 17.2, AU-2).
//!
//! The MAC here is a non-cryptographic placeholder (FNV-1a over key + payload)
//! that exercises the key-id / rotation / revocation interface. It will be
//! replaced by real asymmetric signatures (e.g. Ed25519) without changing the
//! interface.

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

    /// Add or rotate in a key.
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
        Some(mac(secret, payload))
    }

    /// Verify a signature. Verification succeeds on signature match regardless of
    /// revocation; callers consult [`KeyRing::is_revoked`] separately so a revoked
    /// key's old approvals fail closed at validation (PRD 17.3 revocation survives).
    pub fn verify(&self, key_id: &str, payload: &[u8], signature: &str) -> bool {
        match self.secrets.get(key_id) {
            Some(secret) => mac(secret, payload) == signature,
            None => false,
        }
    }
}

fn mac(secret: &str, payload: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in secret.bytes().chain(std::iter::once(0u8)).chain(payload.iter().copied()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    format!("mac1:{h:016x}")
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
}
