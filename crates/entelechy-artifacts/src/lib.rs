//! Artifact identity, schema metadata, lineage and content-addressed storage.
//!
//! PRD v11 references: sections 5.2 (canonical artifacts), 5.9 (schema
//! evolution & digest agility), 17 (storage & immutability), Q22.
//!
//! Every canonical artifact is immutable and content-addressed. Mutable product
//! state is represented as new artifact versions plus append-only events
//! (PRD 17). Each artifact carries a schema identifier, semantic schema version,
//! producer version and canonical serialization version (PRD 5.9), and readers
//! declare a backward-read window that fails closed on unsupported versions.
#![forbid(unsafe_code)]

mod canonical;
mod id;
mod store;

pub use canonical::{serialize_canonical, to_canonical_bytes};
pub use id::{ArtifactId, DigestAlgorithm, IdParseError};
pub use store::{BlobStore, InMemoryStore, StoreError};

use serde::{Deserialize, Serialize};

/// Canonical serialization contract version. Bumped only by a change to the
/// byte-level encoding in [`canonical`] (PRD 5.9). Part of every header so a
/// reader can refuse an encoding it does not understand.
pub const CANONICAL_SERIALIZATION_VERSION: u32 = 1;

/// A semantic version triple for artifact schemas (PRD 5.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SchemaVersion {
    /// Incompatible schema change.
    pub major: u32,
    /// Backward-compatible addition.
    pub minor: u32,
    /// Non-semantic revision.
    pub patch: u32,
}

impl SchemaVersion {
    /// Construct a version.
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self { major, minor, patch }
    }
}

impl std::fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The self-describing header embedded in every canonical artifact (PRD 5.9).
///
/// The header participates in the content hash, so two artifacts produced by
/// different binaries but with identical schema and payload still share an ID
/// only when their headers match — which is what makes fail-closed reads safe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactHeader {
    /// Stable schema identifier, e.g. `"entelechy.goalspec"`.
    pub schema_id: String,
    /// Semantic schema version.
    pub schema_version: SchemaVersion,
    /// Version of the producer that emitted this artifact.
    pub producer_version: String,
    /// Canonical serialization version ([`CANONICAL_SERIALIZATION_VERSION`]).
    pub canonical_version: u32,
    /// Tenant / security-domain identifier. Present even in local mode with a
    /// single default domain (PRD 17.2).
    pub tenant: String,
}

impl ArtifactHeader {
    /// Build a header for the given schema, defaulting canonical version and the
    /// local default tenant.
    pub fn new(
        schema_id: impl Into<String>,
        schema_version: SchemaVersion,
        producer_version: impl Into<String>,
    ) -> Self {
        Self {
            schema_id: schema_id.into(),
            schema_version,
            producer_version: producer_version.into(),
            canonical_version: CANONICAL_SERIALIZATION_VERSION,
            tenant: DEFAULT_TENANT.to_string(),
        }
    }
}

/// The default single security domain used in local mode (PRD 17.2).
pub const DEFAULT_TENANT: &str = "local";

/// A relationship between two artifacts (PRD 5.2, 5.9 migration edges).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LineageKind {
    /// `to` was derived from `from` (e.g. compiled, synthesized).
    DerivedFrom,
    /// `to` is a new lineage branch of `from` after a material change (PRD 5.2).
    BranchOf,
    /// `to` is a deterministic migration of `from` (PRD 5.9). Carries a
    /// semantics-preserving claim relevant to approval survival (14.5).
    MigrationOf { semantics_preserving: bool },
    /// `to` supersedes `from` (e.g. a newer release).
    Supersedes,
}

/// A directed lineage edge between content addresses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageEdge {
    /// Source artifact.
    pub from: ArtifactId,
    /// Target artifact.
    pub to: ArtifactId,
    /// Relationship kind.
    pub kind: LineageKind,
}

/// A typed, content-addressed artifact: header + payload, addressed by the hash
/// of both together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact<T> {
    /// Self-describing header (schema, versions, tenant).
    pub header: ArtifactHeader,
    /// Typed payload.
    pub payload: T,
}

impl<T: Serialize> Artifact<T> {
    /// Wrap a payload with a header.
    pub fn new(header: ArtifactHeader, payload: T) -> Self {
        Self { header, payload }
    }

    /// Content address of this artifact (header included).
    pub fn id(&self) -> Result<ArtifactId, serde_json::Error> {
        ArtifactId::of(self)
    }

    /// Canonical bytes of this artifact.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serialize_canonical(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn artifact_id_includes_header() {
        let h1 = ArtifactHeader::new("entelechy.test", SchemaVersion::new(1, 0, 0), "0.0.0");
        let mut h2 = h1.clone();
        h2.schema_version = SchemaVersion::new(2, 0, 0);
        let a = Artifact::new(h1, json!({"v": 1}));
        let b = Artifact::new(h2, json!({"v": 1}));
        assert_ne!(a.id().unwrap(), b.id().unwrap());
    }
}
