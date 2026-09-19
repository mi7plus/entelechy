//! Effect taxonomy and transactional metadata (PRD 7.6, IR-I10, CD-7).
//!
//! Effect metadata is part of the IR contract, not merely tool documentation.
//! Unknown metadata "remains conservative until attested": capabilities with
//! unknown effects default to external and irreversible (PRD 7.6, CD-7).

use serde::{Deserialize, Serialize};

/// The effect class of a capability invocation (PRD 7.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectClass {
    /// Reads state; no external mutation.
    Read,
    /// Mutates state within the system boundary.
    Write,
    /// Calls out beyond the system boundary (includes every model call — PRD 7.5).
    External,
    /// Cannot be undone once committed.
    Irreversible,
}

impl EffectClass {
    /// Whether this class is "consequential": a write, external or irreversible
    /// effect requiring write-ahead intent (PRD 7.6).
    pub fn is_consequential(self) -> bool {
        !matches!(self, EffectClass::Read)
    }
}

/// How trustworthy an [`EffectMetadata`] is (PRD 7.6, CD-7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttestationLevel {
    /// Declared by an MCP server / OpenAPI doc: an untrusted hint (CD-7).
    UntrustedHint,
    /// Verified by sandbox probing.
    SandboxVerified,
    /// Attested by a signed operator manifest (CD-7).
    OperatorAttested,
}

/// Transactional and reliability metadata for one capability (PRD 7.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectMetadata {
    /// The effect class.
    pub class: EffectClass,
    /// Whether repeating the call with the same operation key is safe.
    pub idempotent: bool,
    /// Whether the effect can be reversed / compensated.
    pub reversible: bool,
    /// Whether the capability supports a dry-run mode (needed for L3 shadow — 14.2).
    pub dry_run_supported: bool,
    /// Whether the provider offers a read-back query to reconcile unknown commits
    /// (PRD 7.6, Q16). Required for automatic retry when not idempotent.
    pub read_back_supported: bool,
    /// How trustworthy this metadata is.
    pub attestation: AttestationLevel,
    /// Stable operation key namespace for this capability (PRD 7.6).
    pub operation_key_namespace: String,
}

impl EffectMetadata {
    /// The conservative default for a capability with unknown effects: external,
    /// irreversible, non-idempotent, untrusted (PRD 7.6, CD-7).
    pub fn conservative_default(operation_key_namespace: impl Into<String>) -> Self {
        Self {
            class: EffectClass::Irreversible,
            idempotent: false,
            reversible: false,
            dry_run_supported: false,
            read_back_supported: false,
            attestation: AttestationLevel::UntrustedHint,
            operation_key_namespace: operation_key_namespace.into(),
        }
    }

    /// Whether this capability is eligible for automatic retry (PRD Q16): it must
    /// support native idempotency keys or a read-back reconciliation query.
    pub fn auto_retry_eligible(&self) -> bool {
        self.idempotent || self.read_back_supported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_effects_default_conservative() {
        let m = EffectMetadata::conservative_default("ns");
        assert_eq!(m.class, EffectClass::Irreversible);
        assert!(!m.auto_retry_eligible());
        assert_eq!(m.attestation, AttestationLevel::UntrustedHint);
    }

    #[test]
    fn reads_are_not_consequential() {
        assert!(!EffectClass::Read.is_consequential());
        assert!(EffectClass::Write.is_consequential());
    }
}
