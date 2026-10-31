//! Recovery integration tests crossing storage and release (PRD 17.3, DR-3).
//!
//! DR-3 requires that a *partial loss* marks the affected run's/release's evidence
//! `evidence_incomplete` and blocks new promotion until it is regenerated or waived.
//! This exercises the whole path across two crates: back up durable state, lose a
//! block in cold storage, restore what survived, assess the evidence state from what
//! is present, and confirm the real `entelechy-release` promotion gate refuses the
//! promotion — even with a recorded override.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use entelechy_artifacts::{
    assess_evidence_state, Artifact, ArtifactHeader, BackupPacket, BlobStore, ErasureLedger,
    EvidenceState, InMemoryStore, SchemaVersion,
};
use entelechy_assurance::{AssuranceReport, SafetyCase};
use entelechy_identity::{
    Approval, ApprovalBinding, FixedClock, KeyRing, Principal, PrincipalKind,
};
use entelechy_release::{
    promote, ApprovalCheck, MaturityLevel, PromotionError, PromotionGate, PromotionOverride,
    ReleaseBundle,
};

/// Build three canonical artifacts in a store and return the backup packet plus the
/// set of content addresses the release depends on (its required evidence).
fn durable_state() -> (BackupPacket, Vec<String>) {
    let mut store = InMemoryStore::new();
    let mk = |payload: serde_json::Value| {
        Artifact::new(
            ArtifactHeader::new("entelechy.design", SchemaVersion::new(1, 0, 0), "0.0.0"),
            payload,
        )
    };
    let id_design = store.put(
        "local",
        mk(serde_json::json!({ "design": "triage" }))
            .canonical_bytes()
            .unwrap(),
    );
    let id_journal = store.put(
        "local",
        mk(serde_json::json!({ "journal": "run-1" }))
            .canonical_bytes()
            .unwrap(),
    );
    let id_holdout = store.put(
        "local",
        mk(serde_json::json!({ "holdout": "sealed" }))
            .canonical_bytes()
            .unwrap(),
    );
    let required = vec![
        id_design.to_string(),
        id_journal.to_string(),
        id_holdout.to_string(),
    ];
    let packet = BackupPacket::backup(&store, &[], &[], &ErasureLedger::new());
    (packet, required)
}

fn release_bundle() -> ReleaseBundle {
    ReleaseBundle {
        design_id: "sha256:d".into(),
        goalspec_id: "sha256:g".into(),
        authority_id: "sha256:a".into(),
        evalcontract_id: "sha256:e".into(),
        safety_case: SafetyCase::default(),
        policy_snapshot_version: 1,
        model_tool_ids: vec!["mock@r1".into()],
        provenance: "study X".into(),
        rollback_target: Some("rel-prev".into()),
        no_rollback_rationale: None,
    }
}

fn approval() -> (KeyRing, ApprovalBinding, Vec<Approval>) {
    let mut ring = KeyRing::new();
    ring.add_key("k", "s");
    let binding: ApprovalBinding = BTreeMap::from([("design".to_string(), "sha256:d".to_string())]);
    let appr = Approval::sign(
        Principal::new("alice", PrincipalKind::Approver),
        binding.clone(),
        "k",
        &ring,
        10,
        Some(10_000),
    )
    .unwrap();
    (ring, binding, vec![appr])
}

#[test]
fn partial_loss_marks_incomplete_and_blocks_promotion() {
    let (packet, required) = durable_state();
    assert_eq!(packet.objects.len(), 3, "three objects backed up");

    // Cold storage loses one block: the backup comes back missing an object. The
    // restore of what survived is itself clean (surviving objects re-hash), but the
    // evidence set is no longer whole.
    let mut lossy = packet.clone();
    let lost = lossy.objects.remove(1); // the journal block is gone
    let survivors = entelechy_artifacts::run_restore_drill(&lossy).expect("survivors restore");
    assert!(survivors.ok, "the surviving objects restore cleanly");
    assert_eq!(survivors.objects_restored, 2);

    // Assess evidence completeness from what is present after the restore (DR-3).
    let present: BTreeSet<String> = lossy
        .objects
        .iter()
        .map(|o| o.address.to_string())
        .collect();
    let state = assess_evidence_state(required.iter().map(|s| s.as_str()), &present);
    assert_eq!(
        state,
        EvidenceState::Incomplete,
        "a lost block ({}) must mark the evidence incomplete",
        lost.address
    );

    // The real promotion gate refuses a promotion backed by incomplete evidence.
    let bundle = release_bundle();
    let (ring, binding, appr) = approval();
    let check = ApprovalCheck {
        approvals: &appr,
        current_binding: &binding,
        keyring: &ring,
        clock: &FixedClock(Some(20)),
    };
    let gate = PromotionGate {
        assurance: &AssuranceReport {
            static_violations: vec![],
            safety_case: SafetyCase::default(),
            policy_snapshot_version: 1,
        },
        holdout_passed: true,
        negative_goal_clean: true,
        evidence_state: state,
    };
    let err = promote(
        &bundle,
        MaturityLevel::L1,
        MaturityLevel::L2,
        &gate,
        &check,
        None,
    )
    .unwrap_err();
    assert_eq!(err, PromotionError::EvidenceIncomplete);

    // Not override-able: a recorded override cannot resurrect lost evidence.
    let ov = PromotionOverride {
        failed_criterion: "evidence".into(),
        rationale: "we really need it".into(),
        approver: "z".into(),
        expires_at: 9_999,
    };
    let err = promote(
        &bundle,
        MaturityLevel::L1,
        MaturityLevel::L2,
        &gate,
        &check,
        Some(ov),
    )
    .unwrap_err();
    assert_eq!(err, PromotionError::EvidenceIncomplete);

    // Once the evidence is regenerated (all required ids present again), the gate
    // passes — the block is specific to the loss, not a permanent veto.
    let whole: BTreeSet<String> = required.iter().cloned().collect();
    let healthy = assess_evidence_state(required.iter().map(|s| s.as_str()), &whole);
    let gate_ok = PromotionGate {
        assurance: &AssuranceReport {
            static_violations: vec![],
            safety_case: SafetyCase::default(),
            policy_snapshot_version: 1,
        },
        holdout_passed: true,
        negative_goal_clean: true,
        evidence_state: healthy,
    };
    let ok = promote(
        &bundle,
        MaturityLevel::L1,
        MaturityLevel::L2,
        &gate_ok,
        &check,
        None,
    );
    assert!(ok.is_ok(), "complete evidence promotes: {ok:?}");
}
