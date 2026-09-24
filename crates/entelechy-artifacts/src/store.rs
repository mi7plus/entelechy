//! Content-addressed storage (PRD 17, 17.2).
//!
//! Storage is keyed by content address. Critically, "authorization is checked
//! before resolving content-addressed blobs; possession of a hash is never
//! sufficient authority to read content" (PRD 17.2). The store therefore takes a
//! requesting tenant and refuses cross-domain reads.

use std::collections::HashMap;

use crate::ArtifactId;

/// Errors from the content-addressed store.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StoreError {
    /// No object exists at the given address.
    #[error("object not found: {0}")]
    NotFound(ArtifactId),
    /// The requesting tenant is not authorized for this object (PRD 17.2).
    #[error("cross-tenant access denied for {0}")]
    Unauthorized(ArtifactId),
    /// Stored bytes did not hash to the address they were filed under.
    #[error("integrity check failed for {0}")]
    IntegrityFailure(ArtifactId),
}

/// A content-addressed blob store. Writes are idempotent (same bytes → same
/// address); the store is append-only in the sense that an address's content
/// never changes.
pub trait BlobStore {
    /// Store raw bytes under a tenant, returning their content address.
    fn put(&mut self, tenant: &str, bytes: Vec<u8>) -> ArtifactId;

    /// Fetch bytes by address, enforcing tenant authorization first (PRD 17.2).
    fn get(&self, tenant: &str, id: &ArtifactId) -> Result<&[u8], StoreError>;

    /// Whether an address exists and is readable by the tenant.
    fn contains(&self, tenant: &str, id: &ArtifactId) -> bool;
}

/// A simple in-process [`BlobStore`] for local mode and tests.
#[derive(Default)]
pub struct InMemoryStore {
    // address -> (tenant, bytes)
    objects: HashMap<ArtifactId, (String, Vec<u8>)>,
}

impl InMemoryStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of stored objects.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether the store holds no objects.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

impl BlobStore for InMemoryStore {
    fn put(&mut self, tenant: &str, bytes: Vec<u8>) -> ArtifactId {
        let id = ArtifactId::of_blob(&bytes);
        self.objects
            .entry(id.clone())
            .or_insert_with(|| (tenant.to_string(), bytes));
        id
    }

    fn get(&self, tenant: &str, id: &ArtifactId) -> Result<&[u8], StoreError> {
        let (owner, bytes) = self
            .objects
            .get(id)
            .ok_or_else(|| StoreError::NotFound(id.clone()))?;
        // Authorization before resolution (PRD 17.2).
        if owner != tenant {
            return Err(StoreError::Unauthorized(id.clone()));
        }
        // Integrity: bytes must still hash to their address.
        if &ArtifactId::of_blob(bytes) != id {
            return Err(StoreError::IntegrityFailure(id.clone()));
        }
        Ok(bytes)
    }

    fn contains(&self, tenant: &str, id: &ArtifactId) -> bool {
        matches!(self.objects.get(id), Some((owner, _)) if owner == tenant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_roundtrip() {
        let mut store = InMemoryStore::new();
        let id = store.put("local", b"hello".to_vec());
        assert_eq!(store.get("local", &id).unwrap(), b"hello");
    }

    #[test]
    fn possession_of_hash_is_not_authority() {
        let mut store = InMemoryStore::new();
        let id = store.put("tenant-a", b"secret".to_vec());
        // Another tenant holds the exact address but is refused (PRD 17.2).
        assert_eq!(
            store.get("tenant-b", &id),
            Err(StoreError::Unauthorized(id.clone()))
        );
        assert!(!store.contains("tenant-b", &id));
    }

    #[test]
    fn put_is_idempotent() {
        let mut store = InMemoryStore::new();
        let a = store.put("local", b"same".to_vec());
        let b = store.put("local", b"same".to_vec());
        assert_eq!(a, b);
        assert_eq!(store.len(), 1);
    }
}
