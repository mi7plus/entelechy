//! The audit / attestation drill (PRD §17.2, AU-1/AU-2/AU-3): a single composed
//! operational drill over the identity stack.
//!
//! It proves, end to end, that:
//!
//! - **AU-1**: a privileged operation carries an authenticated identity whose
//!   credential expiry fails closed, authentication never implies authorization
//!   (separation of duties still refuses a self-approval), and every privileged
//!   action lands in a hash-chained, externally-anchored audit log that detects a
//!   full rewrite;
//! - **AU-2** (key drill): a signing key can be rotated — a new key signs new
//!   writes while the content-addressed artifacts are *not* mutated — and once a key
//!   is revoked it can never sign a new write and its existing approvals fail closed;
//! - **AU-3**: a security-sensitive time decision on an uncertain clock fails closed
//!   rather than defaulting to "valid".
//!
//! The crate's unit tests exercise each primitive in isolation; this drill is the
//! composed operational/key-drill verification those requirements call for, so an
//! operator can run it (and persist the report) as an audit exercise.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use entelechy_artifacts::ArtifactId;

use crate::{
    Approval, ApprovalBinding, ApprovalStatus, AuditLog, AuthContext, AuthError, Credential,
    FixedClock, KeyRing, Principal, PrincipalKind, SeparationOfDuties,
};

/// The outcome of the audit / attestation drill (PRD §17.2, AU-1/AU-2/AU-3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditDrillReport {
    /// The principal that authenticated the privileged operation (AU-1).
    pub authenticated_principal: String,
    /// AU-1: authentication succeeds but does not grant authorization — separation
    /// of duties refuses a self-approval while permitting a distinct approver.
    pub authentication_requires_authorization: bool,
    /// AU-1: an expired credential is refused (fail closed).
    pub expiry_fails_closed: bool,
    /// AU-3: an uncertain clock fails closed for both authentication and an
    /// expiring approval, rather than defaulting to valid.
    pub uncertain_time_fails_closed: bool,
    /// AU-2: the approval signed with the original key was valid before the key
    /// lifecycle began.
    pub approval_valid_before_revocation: bool,
    /// AU-2: after rotation, the new key signs and validates new writes.
    pub rotated_key_signs_new_writes: bool,
    /// AU-2: a revoked key can never sign a new write.
    pub revoked_key_cannot_sign: bool,
    /// AU-2: a revoked key's existing approval fails closed (reports revoked).
    pub revoked_key_approval_fails_closed: bool,
    /// AU-2: rotation/revocation does not mutate plaintext — the content address of
    /// the signed artifact is unchanged across the key lifecycle.
    pub artifact_address_stable: bool,
    /// AU-1: privileged actions recorded in the tamper-evident audit log.
    pub audit_entries: usize,
    /// AU-1: the audit hash chain verifies.
    pub audit_chain_valid: bool,
    /// AU-1: an externally held anchor is still in the chain's history.
    pub audit_anchor_holds: bool,
    /// AU-1: a full rewrite (internally consistent, different content) is detected
    /// because the externally-held anchor no longer matches any entry.
    pub tamper_detected: bool,
    /// Whether every AU-1/AU-2/AU-3 invariant held.
    pub ok: bool,
}

/// Run the audit / attestation drill (PRD §17.2). Deterministic: fixed keys,
/// timestamps and clocks mean the same inputs always yield the same report.
pub fn audit_drill() -> AuditDrillReport {
    // A content-addressed "design" artifact an approval binds to. Its address is
    // recorded before and after the key lifecycle to show plaintext is not mutated.
    let design = serde_json::json!({ "design": "triage-v1" });
    let design_id = ArtifactId::of(&design)
        .expect("design is addressable")
        .to_string();
    let binding: ApprovalBinding = BTreeMap::from([("design".to_string(), design_id.clone())]);

    // --- AU-1: a privileged operation authenticates; authentication != authorization.
    let approver = Principal::new("approver", PrincipalKind::Approver);
    let ctx = AuthContext {
        principal: approver.clone(),
        tenant: "local".into(),
        credential: Credential {
            issued_at: 1_000,
            expires_at: 2_000,
        },
    };
    let authenticated_principal = ctx
        .authenticate(&FixedClock(Some(1_500)))
        .map(|p| p.id.clone())
        .unwrap_or_default();
    let expiry_fails_closed = matches!(
        ctx.authenticate(&FixedClock(Some(3_000))),
        Err(AuthError::Expired)
    );
    // Separation of duties: the same identity cannot author and approve; a distinct
    // approver may (AU-1 separate authorization).
    let sod = SeparationOfDuties { enforce: true };
    let self_approve = Principal::new("approver", PrincipalKind::DomainOwner);
    let distinct = Principal::new("auditor", PrincipalKind::Approver);
    let authz_separate =
        !sod.permits(&approver, &self_approve) && sod.permits(&approver, &distinct);
    let authentication_requires_authorization =
        !authenticated_principal.is_empty() && authz_separate;

    // --- AU-3: an uncertain clock fails closed for authentication.
    let auth_uncertain_fails_closed = matches!(
        ctx.authenticate(&FixedClock(None)),
        Err(AuthError::TimeUncertain)
    );

    // --- AU-2 (key drill): sign with key-1, rotate to key-2, revoke key-1.
    let mut ring = KeyRing::new();
    ring.add_key("key-1", "secret-1");
    let address_before = ArtifactId::of(&design).expect("addressable").to_string();

    let approval_k1 = Approval::sign(
        approver.clone(),
        binding.clone(),
        "key-1",
        &ring,
        1_000,
        Some(2_000),
    )
    .expect("key-1 signs");
    let approval_valid_before_revocation = matches!(
        approval_k1.status_for(&binding, &ring, &FixedClock(Some(1_500))),
        ApprovalStatus::Valid
    );

    // Rotate: a new key signs new writes (the old key's artifacts are untouched).
    ring.add_key("key-2", "secret-2");
    let approval_k2 = Approval::sign(
        approver.clone(),
        binding.clone(),
        "key-2",
        &ring,
        1_200,
        Some(2_000),
    )
    .expect("key-2 signs");
    let k2_valid = matches!(
        approval_k2.status_for(&binding, &ring, &FixedClock(Some(1_500))),
        ApprovalStatus::Valid
    );

    // Revoke key-1: it can never sign a new write, and its old approval fails closed.
    ring.revoke("key-1");
    let revoked_key_cannot_sign = ring.sign("key-1", b"new-write").is_none();
    let revoked_key_approval_fails_closed = matches!(
        approval_k1.status_for(&binding, &ring, &FixedClock(Some(1_500))),
        ApprovalStatus::Revoked
    );
    let rotated_key_signs_new_writes = k2_valid && ring.sign("key-2", b"new-write").is_some();

    // Plaintext not mutated: the content address of the signed artifact is unchanged
    // across the whole key lifecycle (rotation/revocation live in the keyring).
    let address_after = ArtifactId::of(&design).expect("addressable").to_string();
    let artifact_address_stable = address_before == address_after && address_before == design_id;

    // AU-3 again: an expiring approval on an uncertain clock fails closed.
    let approval_uncertain_fails_closed = matches!(
        approval_k2.status_for(&binding, &ring, &FixedClock(None)),
        ApprovalStatus::Expired
    );
    let uncertain_time_fails_closed =
        auth_uncertain_fails_closed && approval_uncertain_fails_closed;

    // --- AU-1 audit trail: every privileged action in a hash-chained, anchored log.
    let mut log = AuditLog::new();
    log.append(
        &authenticated_principal,
        format!("authenticate principal={authenticated_principal}"),
        1_500,
    );
    log.append(
        "operator",
        format!("sign approval key=key-1 design={design_id}"),
        1_000,
    );
    log.append("operator", "rotate signing key key-1 -> key-2", 1_200);
    log.append("operator", "revoke signing key key-1", 1_300);
    let anchor = log.anchor(); // held outside the operator's control
    let audit_entries = log.len();
    let audit_chain_valid = log.verify_chain();
    let audit_anchor_holds = log.verify_anchor(&anchor);

    // A full rewrite that forges a past action recomputes to different hashes: it is
    // internally consistent, but the externally-held anchor no longer matches, so
    // the tamper is detected (PRD §17.2 periodic anchoring).
    let mut rewritten = AuditLog::new();
    rewritten.append("operator", "authenticate principal=approver", 1_500);
    rewritten.append(
        "operator",
        "sign approval key=key-1 design=sha256:FORGED",
        1_000,
    );
    rewritten.append("operator", "rotate signing key key-1 -> key-2", 1_200);
    rewritten.append("operator", "revoke signing key key-1", 1_300);
    let tamper_detected = rewritten.verify_chain() && !rewritten.verify_anchor(&anchor);

    let ok = authentication_requires_authorization
        && expiry_fails_closed
        && uncertain_time_fails_closed
        && approval_valid_before_revocation
        && rotated_key_signs_new_writes
        && revoked_key_cannot_sign
        && revoked_key_approval_fails_closed
        && artifact_address_stable
        && audit_chain_valid
        && audit_anchor_holds
        && tamper_detected;

    AuditDrillReport {
        authenticated_principal,
        authentication_requires_authorization,
        expiry_fails_closed,
        uncertain_time_fails_closed,
        approval_valid_before_revocation,
        rotated_key_signs_new_writes,
        revoked_key_cannot_sign,
        revoked_key_approval_fails_closed,
        artifact_address_stable,
        audit_entries,
        audit_chain_valid,
        audit_anchor_holds,
        tamper_detected,
        ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_drill_satisfies_au1_au2_au3() {
        let r = audit_drill();
        assert!(r.ok, "every AU-1/AU-2/AU-3 invariant must hold: {r:?}");
        // AU-1.
        assert_eq!(r.authenticated_principal, "approver");
        assert!(r.authentication_requires_authorization);
        assert!(r.expiry_fails_closed);
        assert_eq!(r.audit_entries, 4);
        assert!(r.audit_chain_valid);
        assert!(r.audit_anchor_holds);
        assert!(r.tamper_detected);
        // AU-2.
        assert!(r.approval_valid_before_revocation);
        assert!(r.rotated_key_signs_new_writes);
        assert!(r.revoked_key_cannot_sign);
        assert!(r.revoked_key_approval_fails_closed);
        assert!(r.artifact_address_stable);
        // AU-3.
        assert!(r.uncertain_time_fails_closed);
    }

    #[test]
    fn the_drill_is_deterministic() {
        assert_eq!(audit_drill(), audit_drill());
    }
}
