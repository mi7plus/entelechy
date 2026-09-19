//! Principals, authentication context, approval signatures and separation of
//! duties.
//!
//! PRD v11 references: section 5.8 (trust model & principals), 14.5 (approval
//! integrity & staleness), 17.2 (authentication, sessions, key ownership), AU-1
//! (authenticated identity + separate authorization; expiry fails closed), AU-2
//! (rotation/revocation), AU-3 (time source fails closed), Q17 (material change),
//! Q30 (short-lived credentials).
#![forbid(unsafe_code)]

pub mod keyring;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use keyring::KeyRing;

/// The kind of principal (PRD 5.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrincipalKind {
    /// States objectives, approves eval meaning and releases.
    DomainOwner,
    /// Runs studies and deployments.
    Operator,
    /// Signs approval decisions.
    Approver,
    /// The runtime service identity (data plane).
    RuntimeSystem,
    /// A meta-layer agent: proposal-only, no runtime authority (PRD 5.8, 11.9).
    MetaLayerAgent,
    /// An external model/tool provider (untrusted).
    ExternalProvider,
    /// A tool/plugin.
    ToolPlugin,
    /// An end user the released system acts on behalf of (PRD 5.8).
    EndUser,
}

impl PrincipalKind {
    /// Whether this principal may hold runtime authority. Meta-layer agents and
    /// external providers never do (PRD 5.8, 11.9).
    pub fn may_hold_runtime_authority(self) -> bool {
        !matches!(self, PrincipalKind::MetaLayerAgent | PrincipalKind::ExternalProvider)
    }
}

/// An authenticated principal (PRD 5.8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    /// Stable identity string.
    pub id: String,
    /// The principal kind.
    pub kind: PrincipalKind,
}

impl Principal {
    /// Construct a principal.
    pub fn new(id: impl Into<String>, kind: PrincipalKind) -> Self {
        Self { id: id.into(), kind }
    }
}

/// A time source. `now()` returns `None` when the time is unauthenticated or
/// uncertain, which makes security-sensitive decisions fail closed (PRD 17.2,
/// AU-3).
pub trait TimeSource {
    /// Current time in unix seconds, or `None` if uncertain.
    fn now(&self) -> Option<u64>;
}

/// A fixed clock for tests / deterministic contexts.
pub struct FixedClock(pub Option<u64>);
impl TimeSource for FixedClock {
    fn now(&self) -> Option<u64> {
        self.0
    }
}

/// A short-lived credential (PRD 17.2, Q30: service tokens ≤ 1 hour).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    /// Issued-at unix seconds.
    pub issued_at: u64,
    /// Expiry unix seconds.
    pub expires_at: u64,
}

impl Credential {
    /// Whether the credential is valid at `now` (strictly before expiry).
    pub fn is_valid_at(&self, now: u64) -> bool {
        now < self.expires_at
    }
}

/// Errors from authentication.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    /// The credential has expired.
    #[error("credential expired")]
    Expired,
    /// The time source is uncertain, so the decision fails closed (AU-3).
    #[error("time source uncertain; failing closed")]
    TimeUncertain,
}

/// An explicit authentication context carried on every privileged operation
/// (PRD 17.2, AU-1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthContext {
    /// The authenticated principal.
    pub principal: Principal,
    /// Tenant / security domain (PRD 17.2).
    pub tenant: String,
    /// The presented credential.
    pub credential: Credential,
}

impl AuthContext {
    /// Authenticate against a time source, failing closed on expiry or uncertain
    /// time (AU-1, AU-3). Authentication success never implies authorization
    /// (PRD 17.2) — callers still consult the policy engine.
    pub fn authenticate(&self, clock: &dyn TimeSource) -> Result<&Principal, AuthError> {
        let now = clock.now().ok_or(AuthError::TimeUncertain)?;
        if !self.credential.is_valid_at(now) {
            return Err(AuthError::Expired);
        }
        Ok(&self.principal)
    }
}

/// The set of immutable artifact hashes an approval binds to (PRD 14.5, Q17).
///
/// Material changes to any of these invalidate the approval: the Design IR,
/// AuthorityEnvelope, PolicySnapshot, EvalContract, model/tool identity or
/// required evidence (Q17).
pub type ApprovalBinding = BTreeMap<String, String>;

/// A signed decision over immutable artifact hashes (PRD 14.5). Not a
/// free-floating yes/no flag: it binds to exact content hashes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    /// The approver.
    pub approver: Principal,
    /// The hashes this approval is bound to (role → content hash).
    pub binding: ApprovalBinding,
    /// The signing key id (for rotation/revocation — PRD 5.9).
    pub key_id: String,
    /// The signature over the canonical binding.
    pub signature: String,
    /// When the approval was made (unix seconds).
    pub approved_at: u64,
    /// Optional expiry (PRD 14.5 approvals may carry expiry).
    pub expires_at: Option<u64>,
}

/// The validity status of an approval at promotion time (PRD 14.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalStatus {
    /// Valid and current.
    Valid,
    /// A bound artifact changed materially (Q17) — re-approval required.
    Stale {
        /// The role whose hash no longer matches.
        role: String,
    },
    /// The approval has expired.
    Expired,
    /// The signing key was revoked.
    Revoked,
    /// The signature does not verify.
    BadSignature,
}

impl Approval {
    /// Create and sign an approval over a binding (PRD 14.5).
    pub fn sign(
        approver: Principal,
        binding: ApprovalBinding,
        key_id: impl Into<String>,
        keyring: &KeyRing,
        approved_at: u64,
        expires_at: Option<u64>,
    ) -> Option<Self> {
        let key_id = key_id.into();
        let payload = signing_payload(&approver, &binding, &key_id);
        let signature = keyring.sign(&key_id, &payload)?;
        Some(Self {
            approver,
            binding,
            key_id,
            signature,
            approved_at,
            expires_at,
        })
    }

    /// Revalidate this approval immediately before promotion (PRD 14.5). Checks
    /// signature, revocation, expiry (fail closed on uncertain time), and that
    /// every bound hash still matches the current artifacts (Q17).
    pub fn status_for(
        &self,
        current: &ApprovalBinding,
        keyring: &KeyRing,
        clock: &dyn TimeSource,
    ) -> ApprovalStatus {
        // Signature first.
        let payload = signing_payload(&self.approver, &self.binding, &self.key_id);
        if !keyring.verify(&self.key_id, &payload, &self.signature) {
            return ApprovalStatus::BadSignature;
        }
        // Revoked keys fail closed (PRD 17.3 revocation survives restore).
        if keyring.is_revoked(&self.key_id) {
            return ApprovalStatus::Revoked;
        }
        // Expiry with a fail-closed clock (AU-3).
        if let Some(exp) = self.expires_at {
            match clock.now() {
                Some(now) if now < exp => {}
                _ => return ApprovalStatus::Expired,
            }
        }
        // Material change: every bound hash must still match (Q17).
        for (role, hash) in &self.binding {
            if current.get(role) != Some(hash) {
                return ApprovalStatus::Stale { role: role.clone() };
            }
        }
        // Also detect newly added required roles not covered by the approval.
        for role in current.keys() {
            if !self.binding.contains_key(role) {
                return ApprovalStatus::Stale { role: role.clone() };
            }
        }
        ApprovalStatus::Valid
    }
}

fn signing_payload(approver: &Principal, binding: &ApprovalBinding, key_id: &str) -> Vec<u8> {
    let doc = serde_json::json!({
        "approver": approver,
        "binding": binding,
        "key_id": key_id,
    });
    entelechy_artifacts::to_canonical_bytes(&doc)
}

/// Separation-of-duties policy (PRD 14.5): the same principal may be prohibited
/// from authoring and approving designated high-risk changes.
#[derive(Clone, Copy, Debug, Default)]
pub struct SeparationOfDuties {
    /// Whether author≠approver is required (high-risk changes).
    pub enforce: bool,
}

impl SeparationOfDuties {
    /// Whether an author/approver pair is permitted under this policy.
    pub fn permits(&self, author: &Principal, approver: &Principal) -> bool {
        !self.enforce || author.id != approver.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approver() -> Principal {
        Principal::new("alice", PrincipalKind::Approver)
    }

    fn binding() -> ApprovalBinding {
        BTreeMap::from([
            ("design".to_string(), "sha256:d".to_string()),
            ("authority".to_string(), "sha256:a".to_string()),
        ])
    }

    fn ring() -> KeyRing {
        let mut r = KeyRing::new();
        r.add_key("key-1", "top-secret");
        r
    }

    #[test]
    fn credential_expiry_fails_closed() {
        let ctx = AuthContext {
            principal: approver(),
            tenant: "local".into(),
            credential: Credential { issued_at: 0, expires_at: 100 },
        };
        assert!(ctx.authenticate(&FixedClock(Some(50))).is_ok());
        assert_eq!(ctx.authenticate(&FixedClock(Some(200))), Err(AuthError::Expired));
        // Uncertain time fails closed (AU-3).
        assert_eq!(ctx.authenticate(&FixedClock(None)), Err(AuthError::TimeUncertain));
    }

    #[test]
    fn valid_approval_roundtrip() {
        let ring = ring();
        let a = Approval::sign(approver(), binding(), "key-1", &ring, 10, Some(1000)).unwrap();
        assert_eq!(a.status_for(&binding(), &ring, &FixedClock(Some(20))), ApprovalStatus::Valid);
    }

    #[test]
    fn material_change_invalidates() {
        let ring = ring();
        let a = Approval::sign(approver(), binding(), "key-1", &ring, 10, None).unwrap();
        let mut changed = binding();
        changed.insert("design".into(), "sha256:DIFFERENT".into());
        assert_eq!(
            a.status_for(&changed, &ring, &FixedClock(Some(20))),
            ApprovalStatus::Stale { role: "design".into() }
        );
    }

    #[test]
    fn expired_and_revoked_and_uncertain_fail_closed() {
        let mut ring = ring();
        let a = Approval::sign(approver(), binding(), "key-1", &ring, 10, Some(100)).unwrap();
        // Expired.
        assert_eq!(a.status_for(&binding(), &ring, &FixedClock(Some(200))), ApprovalStatus::Expired);
        // Uncertain time on an expiring approval fails closed.
        assert_eq!(a.status_for(&binding(), &ring, &FixedClock(None)), ApprovalStatus::Expired);
        // Revoked key.
        ring.revoke("key-1");
        assert_eq!(a.status_for(&binding(), &ring, &FixedClock(Some(20))), ApprovalStatus::Revoked);
    }

    #[test]
    fn separation_of_duties() {
        let sod = SeparationOfDuties { enforce: true };
        let author = Principal::new("alice", PrincipalKind::DomainOwner);
        let self_approve = Principal::new("alice", PrincipalKind::Approver);
        let other = Principal::new("bob", PrincipalKind::Approver);
        assert!(!sod.permits(&author, &self_approve));
        assert!(sod.permits(&author, &other));
    }

    #[test]
    fn meta_layer_holds_no_runtime_authority() {
        assert!(!PrincipalKind::MetaLayerAgent.may_hold_runtime_authority());
        assert!(PrincipalKind::RuntimeSystem.may_hold_runtime_authority());
    }
}
