//! Content-addressed artifact identifiers with digest agility (PRD 5.9, Q22).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

use crate::canonical;

/// Digest algorithm identifier. Carried in every [`ArtifactId`] so the hash
/// function "can be replaced without ambiguity" (PRD 5.9, digest agility).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DigestAlgorithm {
    /// SHA-256, the 1.x default (Q22).
    Sha256,
}

impl DigestAlgorithm {
    /// The stable wire/string tag for this algorithm (used in the ID prefix).
    pub fn tag(self) -> &'static str {
        match self {
            DigestAlgorithm::Sha256 => "sha256",
        }
    }
}

impl fmt::Display for DigestAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

/// A content address: `<algorithm>:<hex-digest>`.
///
/// Two artifacts with the same canonical bytes share an ID; any material change
/// produces a new one. This is the substrate for immutability and lineage
/// (PRD 5.2, 17).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtifactId {
    /// Digest algorithm used to compute [`ArtifactId::digest`].
    pub algorithm: DigestAlgorithm,
    /// Lowercase hex-encoded digest of the canonical bytes.
    pub digest: String,
}

impl ArtifactId {
    /// Compute an ID over already-canonical bytes.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        ArtifactId {
            algorithm: DigestAlgorithm::Sha256,
            digest: hex::encode(hasher.finalize()),
        }
    }

    /// Compute an ID by canonicalizing (RFC 8785) then hashing a value.
    pub fn of<T: Serialize>(value: &T) -> Result<Self, serde_json::Error> {
        let bytes = canonical::serialize_canonical(value)?;
        Ok(Self::from_canonical_bytes(&bytes))
    }

    /// Compute an ID over raw blob bytes. Blobs are hashed as raw bytes, not
    /// canonicalized (PRD Q22).
    pub fn of_blob(bytes: &[u8]) -> Self {
        Self::from_canonical_bytes(bytes)
    }

    /// Parse from the `<algorithm>:<hex>` wire form.
    pub fn parse(s: &str) -> Result<Self, IdParseError> {
        let (algo, digest) = s.split_once(':').ok_or(IdParseError::MissingSeparator)?;
        let algorithm = match algo {
            "sha256" => DigestAlgorithm::Sha256,
            other => return Err(IdParseError::UnknownAlgorithm(other.to_string())),
        };
        let expected_len = match algorithm {
            DigestAlgorithm::Sha256 => 64,
        };
        if digest.len() != expected_len || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(IdParseError::MalformedDigest);
        }
        Ok(ArtifactId {
            algorithm,
            digest: digest.to_ascii_lowercase(),
        })
    }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algorithm, self.digest)
    }
}

/// Errors from parsing an [`ArtifactId`] wire string.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdParseError {
    /// The `algorithm:digest` separator was absent.
    #[error("artifact id missing ':' separator")]
    MissingSeparator,
    /// The algorithm tag is not recognized by this reader.
    #[error("unknown digest algorithm: {0}")]
    UnknownAlgorithm(String),
    /// The digest was the wrong length or contained non-hex characters.
    #[error("malformed digest")]
    MalformedDigest,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn id_is_stable_across_key_order() {
        let a = ArtifactId::of(&json!({"x": 1, "y": 2})).unwrap();
        let b = ArtifactId::of(&json!({"y": 2, "x": 1})).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn display_and_parse_roundtrip() {
        let id = ArtifactId::of(&json!({"hello": "world"})).unwrap();
        let text = id.to_string();
        assert!(text.starts_with("sha256:"));
        assert_eq!(ArtifactId::parse(&text).unwrap(), id);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert_eq!(
            ArtifactId::parse("nope"),
            Err(IdParseError::MissingSeparator)
        );
        assert!(matches!(
            ArtifactId::parse("md5:abc"),
            Err(IdParseError::UnknownAlgorithm(_))
        ));
        assert_eq!(
            ArtifactId::parse("sha256:xyz"),
            Err(IdParseError::MalformedDigest)
        );
    }
}
