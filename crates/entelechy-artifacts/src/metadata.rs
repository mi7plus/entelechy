//! Metadata / lineage store (PRD 17): mutable product state as append-only events
//! plus artifact versions.
//!
//! "Mutable product state is represented as new artifact versions plus append-only
//! events" (PRD 17). The metadata store is SQLite in local mode and Postgres in
//! cluster mode (PRD 17 storage table); this module defines the [`MetadataStore`]
//! trait and an in-process reference. Every event carries a tenant/security-domain
//! id and reads are tenant-scoped (PRD 17.2: authorization before resolution).

use serde::{Deserialize, Serialize};

use crate::ArtifactId;

/// An append-only metadata event (PRD 17).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Global sequence number (append order).
    pub seq: u64,
    /// Tenant / security domain (PRD 17.2).
    pub tenant: String,
    /// Event kind, e.g. `goalspec.signed`, `release.promoted`.
    pub kind: String,
    /// The artifact this event concerns, if any (a new version).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<ArtifactId>,
    /// Event payload.
    pub data: serde_json::Value,
}

/// An append-only metadata/lineage store (PRD 17). Append-only: events are never
/// mutated or deleted, only added.
pub trait MetadataStore {
    /// Append an event, returning its sequence number.
    fn append_event(
        &mut self,
        tenant: &str,
        kind: &str,
        artifact_id: Option<ArtifactId>,
        data: serde_json::Value,
    ) -> u64;

    /// All events for a tenant, in append order (tenant-scoped — PRD 17.2).
    fn events(&self, tenant: &str) -> Vec<&Event>;

    /// The most recent artifact version of a given kind for a tenant.
    fn latest_version(&self, tenant: &str, kind: &str) -> Option<&ArtifactId>;
}

/// An in-process [`MetadataStore`] for local mode and tests.
#[derive(Default)]
pub struct InMemoryMetadataStore {
    events: Vec<Event>,
}

impl InMemoryMetadataStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Total number of events across all tenants.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the store holds no events.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl MetadataStore for InMemoryMetadataStore {
    fn append_event(
        &mut self,
        tenant: &str,
        kind: &str,
        artifact_id: Option<ArtifactId>,
        data: serde_json::Value,
    ) -> u64 {
        let seq = self.events.len() as u64;
        self.events.push(Event {
            seq,
            tenant: tenant.to_string(),
            kind: kind.to_string(),
            artifact_id,
            data,
        });
        seq
    }

    fn events(&self, tenant: &str) -> Vec<&Event> {
        // Tenant scoping is an authorization boundary (PRD 17.2): a caller only
        // sees its own domain's events.
        self.events.iter().filter(|e| e.tenant == tenant).collect()
    }

    fn latest_version(&self, tenant: &str, kind: &str) -> Option<&ArtifactId> {
        self.events
            .iter()
            .rev()
            .find(|e| e.tenant == tenant && e.kind == kind && e.artifact_id.is_some())
            .and_then(|e| e.artifact_id.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_are_append_only_and_sequenced() {
        let mut m = InMemoryMetadataStore::new();
        let s0 = m.append_event("local", "goalspec.signed", None, json!({"v": 1}));
        let s1 = m.append_event("local", "release.promoted", None, json!({"level": "L2"}));
        assert_eq!((s0, s1), (0, 1));
        assert_eq!(m.events("local").len(), 2);
    }

    #[test]
    fn reads_are_tenant_scoped() {
        let mut m = InMemoryMetadataStore::new();
        m.append_event("tenant-a", "x", None, json!({}));
        m.append_event("tenant-b", "x", None, json!({}));
        // A caller only sees its own tenant's events (PRD 17.2).
        assert_eq!(m.events("tenant-a").len(), 1);
        assert_eq!(m.events("tenant-b").len(), 1);
        assert_eq!(m.events("tenant-c").len(), 0);
    }

    #[test]
    fn latest_version_tracks_newest_of_a_kind() {
        let mut m = InMemoryMetadataStore::new();
        let v1 = ArtifactId::of(&json!({"design": 1})).unwrap();
        let v2 = ArtifactId::of(&json!({"design": 2})).unwrap();
        m.append_event("local", "design", Some(v1.clone()), json!({}));
        m.append_event("local", "design", Some(v2.clone()), json!({}));
        assert_eq!(m.latest_version("local", "design"), Some(&v2));
        assert_eq!(m.latest_version("local", "missing"), None);
        // Not visible cross-tenant.
        assert_eq!(m.latest_version("other", "design"), None);
    }
}
