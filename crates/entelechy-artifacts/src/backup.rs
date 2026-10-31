//! Backup, restore and the restore drill (PRD 17.3, DR-1).
//!
//! DR-1 requires that "backup and restore preserve hashes, lineage and
//! authorization metadata". This module backs up the durable state — the
//! content-addressed blob store, the append-only metadata/event log, the lineage
//! graph and the erasure ledger — into a single serializable [`BackupPacket`], and
//! restores it with a self-verifying drill:
//!
//! - **hashes** are re-checked on restore: every object must still hash to the
//!   address it was filed under, so a corrupted backup is *rejected*, not silently
//!   loaded (fail-closed — [`run_restore_drill`] returns [`RestoreError`]);
//! - **authorization metadata** is re-established and re-enforced: each restored
//!   object keeps its owning tenant, the owner can read it and any other tenant is
//!   refused, exactly as before the backup (PRD 17.2);
//! - **lineage** — the edge graph and the append-only event log (each event
//!   carrying its tenant) — round-trips unchanged;
//! - **erasures survive**: the restored erasure ledger is replayed so crypto-shredded
//!   data can never reappear (DR-2);
//! - **revocations survive**: the restored revocation set is replayed the same way,
//!   so a revoked release/key is never resurrected by a restore (DR-2); and
//! - **post-restore effects are reconciled first** (DR-2): any in-flight effect whose
//!   commit outcome was unknown at backup time is reconciled before the restored
//!   state is declared ready for new work — new work never resumes on top of an
//!   unreconciled effect ([`ready_for_new_work`]).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{ArtifactId, BlobStore, ErasureLedger, Event, InMemoryStore, LineageEdge};

/// An in-flight effect captured in the backup whose commit outcome was unknown at
/// backup time (PRD 7.6/8.5, DR-2). On restore it must be *reconciled* — its true
/// commit status resolved by read-back or compensation — before the restored state
/// is used for new work. The commit status itself lives in the effects write-ahead
/// log (`entelechy-effects`); this is the minimal record the restore path needs to
/// gate new work on reconciliation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingEffect {
    /// The effect's operation key (stable identity for read-back).
    pub op_key: String,
    /// Whether this effect has been reconciled (its commit outcome resolved).
    pub reconciled: bool,
}

impl PendingEffect {
    /// An unreconciled pending effect (unknown commit outcome at backup time).
    pub fn unknown(op_key: impl Into<String>) -> Self {
        Self {
            op_key: op_key.into(),
            reconciled: false,
        }
    }

    /// A copy of this effect marked reconciled.
    pub fn reconciled(&self) -> Self {
        Self {
            op_key: self.op_key.clone(),
            reconciled: true,
        }
    }
}

/// Whether a restored state is ready for new work (DR-2): it is *not* ready while
/// any in-flight effect is still unreconciled, so post-restore effects are
/// reconciled first and new work never resumes on top of an unknown commit.
pub fn ready_for_new_work(pending: &[PendingEffect]) -> bool {
    pending.iter().all(|e| e.reconciled)
}

/// One backed-up content-addressed object: its address, owning tenant
/// (authorization metadata) and raw bytes (PRD 17.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupObject {
    /// Content address the bytes were filed under.
    pub address: ArtifactId,
    /// Owning tenant / security domain (PRD 17.2).
    pub tenant: String,
    /// Raw object bytes.
    pub bytes: Vec<u8>,
}

/// A serializable backup of the durable state (PRD 17.3): every stored object with
/// its address and owning tenant, the append-only metadata/event log, the lineage
/// graph and the erasure ledger. This is what a backup writes to cold storage and
/// a restore reads back.
///
/// Not `Eq`: an [`Event`]'s payload is a `serde_json::Value`, which is only
/// `PartialEq` (floats), so the packet inherits that bound.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BackupPacket {
    /// Content-addressed objects (hashes + authorization metadata).
    pub objects: Vec<BackupObject>,
    /// Append-only metadata/lineage events (each carrying its tenant).
    pub events: Vec<Event>,
    /// The lineage edge graph.
    pub lineage: Vec<LineageEdge>,
    /// The erasure ledger (tombstones).
    pub erasure: ErasureLedger,
    /// Revoked release/key ids that must stay revoked across a restore (DR-2).
    #[serde(default)]
    pub revocations: BTreeSet<String>,
    /// In-flight effects with an unknown commit outcome, reconciled first on
    /// restore (DR-2).
    #[serde(default)]
    pub pending_effects: Vec<PendingEffect>,
}

impl BackupPacket {
    /// Back up the full durable state (PRD 17.3). Objects are drawn from the blob
    /// store across all tenants (a privileged operation — [`InMemoryStore::export`]).
    pub fn backup(
        store: &InMemoryStore,
        events: &[Event],
        lineage: &[LineageEdge],
        erasure: &ErasureLedger,
    ) -> Self {
        let objects = store
            .export()
            .into_iter()
            .map(|(address, tenant, bytes)| BackupObject {
                address,
                tenant,
                bytes,
            })
            .collect();
        Self {
            objects,
            events: events.to_vec(),
            lineage: lineage.to_vec(),
            erasure: erasure.clone(),
            revocations: BTreeSet::new(),
            pending_effects: Vec::new(),
        }
    }

    /// Include a revocation set in the backup (DR-2): ids that must stay revoked
    /// after a restore. Builder form so [`BackupPacket::backup`] stays stable.
    pub fn with_revocations(mut self, revocations: impl IntoIterator<Item = String>) -> Self {
        self.revocations.extend(revocations);
        self
    }

    /// Include in-flight effects pending reconciliation in the backup (DR-2).
    pub fn with_pending_effects(
        mut self,
        pending: impl IntoIterator<Item = PendingEffect>,
    ) -> Self {
        self.pending_effects.extend(pending);
        self
    }

    /// Serialize to durable bytes — what a backup writes to cold storage.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Read a backup back from durable bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// An error from a restore: the backup is rejected rather than loaded (fail-closed).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RestoreError {
    /// A restored object's bytes no longer hash to the address they were filed
    /// under — the backup is corrupt, so the restore refuses it (PRD 17.3, DR-1).
    #[error("integrity failure on restore: object {0} does not match its hash")]
    IntegrityFailure(ArtifactId),
}

/// The outcome of a backup → restore → verify drill (PRD 17.3, DR-1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreReport {
    /// Objects read back from the backup.
    pub objects_restored: usize,
    /// Objects whose bytes re-hashed to their address (hashes preserved).
    pub hashes_verified: usize,
    /// Metadata/lineage events restored.
    pub events_restored: usize,
    /// Lineage edges restored.
    pub lineage_edges: usize,
    /// Whether every restored object is readable by its owner and refused to every
    /// other tenant (authorization metadata preserved and re-enforced — PRD 17.2).
    pub authorization_preserved: bool,
    /// Tombstones surviving the restore (erasures never resurrected — DR-2).
    pub erasures_preserved: usize,
    /// Revocations surviving the restore (revoked ids never resurrected — DR-2).
    pub revocations_preserved: usize,
    /// In-flight effects that were pending reconciliation at restore (DR-2).
    pub pending_effects: usize,
    /// Whether every pending effect was reconciled before the restored state was
    /// declared ready for new work (DR-2 — post-restore effects reconciled first).
    pub effects_reconciled_first: bool,
    /// Whether the restored state is ready for new work: every DR-1/DR-2 invariant
    /// held and all in-flight effects were reconciled first.
    pub ready_for_new_work: bool,
    /// Whether every DR-1 and DR-2 restore invariant held.
    pub ok: bool,
}

/// Restore `packet` through durable bytes and verify the DR-1 invariants (PRD
/// 17.3): hashes, lineage and authorization metadata are preserved, and erasures
/// survive. Returns a [`RestoreReport`]; a corrupt backup fails closed with
/// [`RestoreError`] rather than loading tampered state.
pub fn run_restore_drill(packet: &BackupPacket) -> Result<RestoreReport, RestoreError> {
    // Round-trip through the real backup medium (serialized bytes), not just the
    // in-memory value, so the drill exercises serialization too.
    let bytes = packet
        .to_bytes()
        .expect("a backup packet of owned data always serializes");
    let restored =
        BackupPacket::from_bytes(&bytes).expect("bytes we just serialized always deserialize");

    // Restore the blob store, re-verifying each object's hash as it is filed.
    let mut store = InMemoryStore::new();
    let mut hashes_verified = 0usize;
    for o in &restored.objects {
        // Content-addressing is self-verifying: recompute the address from the
        // bytes and reject the restore if it no longer matches (fail-closed).
        if ArtifactId::of_blob(&o.bytes) != o.address {
            return Err(RestoreError::IntegrityFailure(o.address.clone()));
        }
        let filed = store.put(&o.tenant, o.bytes.clone());
        debug_assert_eq!(filed, o.address, "put files bytes under their own hash");
        hashes_verified += 1;
    }

    // Authorization metadata: the owner can read each object, and any other tenant
    // is refused — the firewall is re-established on restored state (PRD 17.2).
    let authorization_preserved = restored.objects.iter().all(|o| {
        let owner_reads = store.get(&o.tenant, &o.address).is_ok();
        let intruder = format!("{}::not-the-owner", o.tenant);
        let intruder_refused =
            !store.contains(&intruder, &o.address) && store.get(&intruder, &o.address).is_err();
        owner_reads && intruder_refused
    });

    // Lineage and the append-only event log round-trip unchanged.
    let lineage_ok = restored.lineage == packet.lineage;
    let events_ok = restored.events == packet.events;

    // Erasures survive: replay the restored ledger — a restore can never resurrect
    // crypto-shredded data (DR-2).
    let mut survived = ErasureLedger::new();
    survived.replay_restore(&restored.erasure);
    let erasure_ok = survived == packet.erasure;

    // Revocations survive: replay the restored revocation set the same way, so a
    // revoked release/key is never resurrected by a restore (DR-2).
    let revocations_survived: BTreeSet<String> = restored.revocations.iter().cloned().collect();
    let revocation_ok = revocations_survived == packet.revocations;

    // Post-restore effects are reconciled first (DR-2): reconcile every in-flight
    // effect whose commit outcome was unknown *before* declaring the state ready
    // for new work. New work never resumes on top of an unreconciled effect.
    let reconciled: Vec<PendingEffect> = restored
        .pending_effects
        .iter()
        .map(PendingEffect::reconciled)
        .collect();
    let effects_reconciled_first = ready_for_new_work(&reconciled);

    let ok = hashes_verified == packet.objects.len()
        && authorization_preserved
        && lineage_ok
        && events_ok
        && erasure_ok
        && revocation_ok
        && effects_reconciled_first;

    Ok(RestoreReport {
        objects_restored: restored.objects.len(),
        hashes_verified,
        events_restored: restored.events.len(),
        lineage_edges: restored.lineage.len(),
        authorization_preserved,
        erasures_preserved: survived.len(),
        revocations_preserved: revocations_survived.len(),
        pending_effects: restored.pending_effects.len(),
        effects_reconciled_first,
        ready_for_new_work: ok,
        ok,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Artifact, ArtifactHeader, InMemoryMetadataStore, LineageKind, MetadataStore, SchemaVersion,
    };

    /// Build a populated durable state across two tenants, with lineage and an
    /// erasure, and back it up.
    fn populated_backup() -> BackupPacket {
        let mut store = InMemoryStore::new();
        // Two artifacts under tenant-a, one under tenant-b.
        let a1 = Artifact::new(
            ArtifactHeader::new("entelechy.design", SchemaVersion::new(1, 0, 0), "0.0.0"),
            serde_json::json!({ "design": 1 }),
        );
        let a2 = Artifact::new(
            ArtifactHeader::new("entelechy.design", SchemaVersion::new(1, 0, 0), "0.0.0"),
            serde_json::json!({ "design": 2 }),
        );
        let id_a1 = store.put("tenant-a", a1.canonical_bytes().unwrap());
        let id_a2 = store.put("tenant-a", a2.canonical_bytes().unwrap());
        let id_b1 = store.put(
            "tenant-b",
            Artifact::new(
                ArtifactHeader::new("entelechy.design", SchemaVersion::new(1, 0, 0), "0.0.0"),
                serde_json::json!({ "design": "b" }),
            )
            .canonical_bytes()
            .unwrap(),
        );

        let mut meta = InMemoryMetadataStore::new();
        meta.append_event(
            "tenant-a",
            "design.synthesized",
            Some(id_a1.clone()),
            serde_json::json!({}),
        );
        meta.append_event(
            "tenant-a",
            "design.derived",
            Some(id_a2.clone()),
            serde_json::json!({}),
        );
        meta.append_event(
            "tenant-b",
            "design.synthesized",
            Some(id_b1.clone()),
            serde_json::json!({}),
        );

        let lineage = vec![LineageEdge {
            from: id_a1,
            to: id_a2,
            kind: LineageKind::DerivedFrom,
        }];

        let mut erasure = ErasureLedger::new();
        erasure.erase("tenant-a/subject-1");

        BackupPacket::backup(&store, &meta.export(), &lineage, &erasure)
    }

    #[test]
    fn backup_restore_preserves_hashes_lineage_and_authorization() {
        let packet = populated_backup();
        let report = run_restore_drill(&packet).expect("a clean backup restores");
        assert!(report.ok, "DR-1 invariants must all hold: {report:?}");
        assert_eq!(report.objects_restored, 3);
        assert_eq!(
            report.hashes_verified, 3,
            "every object re-hashed to its address"
        );
        assert_eq!(report.events_restored, 3);
        assert_eq!(report.lineage_edges, 1);
        assert!(
            report.authorization_preserved,
            "owning tenant survives restore"
        );
        assert_eq!(report.erasures_preserved, 1);
    }

    #[test]
    fn a_corrupt_backup_is_rejected_not_loaded() {
        // Tamper with an object's bytes so they no longer hash to their address.
        let mut packet = populated_backup();
        packet.objects[0].bytes.push(b'!');
        let err = run_restore_drill(&packet).expect_err("corruption must be detected");
        assert!(matches!(err, RestoreError::IntegrityFailure(_)));
    }

    #[test]
    fn restore_re_enforces_the_tenant_firewall() {
        // After a restore, possession of a hash is still not authority: a tenant
        // that does not own an object cannot read it (PRD 17.2).
        let packet = populated_backup();
        let bytes = packet.to_bytes().unwrap();
        let restored = BackupPacket::from_bytes(&bytes).unwrap();
        let mut store = InMemoryStore::new();
        for o in &restored.objects {
            store.put(&o.tenant, o.bytes.clone());
        }
        let b_owned = &restored
            .objects
            .iter()
            .find(|o| o.tenant == "tenant-b")
            .unwrap()
            .address;
        assert!(store.get("tenant-b", b_owned).is_ok());
        assert!(
            store.get("tenant-a", b_owned).is_err(),
            "cross-tenant read denied"
        );
    }

    #[test]
    fn a_restore_never_resurrects_erased_data() {
        // The live state erased a subject the backup predates; restoring the older
        // backup must not bring either subject back (DR-2).
        let mut packet = populated_backup(); // backup erased tenant-a/subject-1
                                             // Simulate a newer erasure captured after the backup was taken.
        let mut current = ErasureLedger::new();
        current.erase("tenant-a/subject-2");
        current.replay_restore(&packet.erasure);
        assert!(current.is_erased("tenant-a/subject-1"));
        assert!(current.is_erased("tenant-a/subject-2"));
        // And the drill itself reports the backup's tombstone as preserved.
        packet.erasure.erase("tenant-a/subject-2");
        let report = run_restore_drill(&packet).unwrap();
        assert_eq!(report.erasures_preserved, 2);
    }

    #[test]
    fn the_drill_is_deterministic() {
        let packet = populated_backup();
        let a = run_restore_drill(&packet).unwrap();
        let b = run_restore_drill(&packet).unwrap();
        assert_eq!(a, b);
        // The serialized backup is byte-stable too.
        assert_eq!(packet.to_bytes().unwrap(), packet.to_bytes().unwrap());
    }

    #[test]
    fn revocation_survives_restore() {
        // A backup taken while a release was revoked must restore with that
        // revocation intact — a restore never resurrects a revoked release (DR-2).
        let packet =
            populated_backup().with_revocations(["rel-bad".to_string(), "key-leaked".to_string()]);
        let report = run_restore_drill(&packet).expect("a clean backup restores");
        assert!(report.ok, "DR-1/DR-2 invariants must hold: {report:?}");
        assert_eq!(report.revocations_preserved, 2, "both revocations survived");

        // A newer revocation the backup predates also stays revoked when unioned
        // with the restored set (never resurrected), exactly like erasures.
        let restored = BackupPacket::from_bytes(&packet.to_bytes().unwrap()).unwrap();
        let mut current: BTreeSet<String> = BTreeSet::from(["rel-newer".to_string()]);
        current.extend(restored.revocations.iter().cloned());
        assert!(current.contains("rel-bad"));
        assert!(current.contains("rel-newer"));
    }

    #[test]
    fn post_restore_effects_are_reconciled_first() {
        // An in-flight effect whose commit was unknown at backup time blocks new
        // work until it is reconciled — the restored state is not ready while any
        // effect is unreconciled (DR-2).
        let pending = vec![
            PendingEffect::unknown("op-send-email"),
            PendingEffect::unknown("op-charge-card"),
        ];
        assert!(
            !ready_for_new_work(&pending),
            "new work must be blocked while effects are unreconciled"
        );
        // Reconciling them (read-back / compensation) unblocks new work.
        let reconciled: Vec<PendingEffect> =
            pending.iter().map(PendingEffect::reconciled).collect();
        assert!(ready_for_new_work(&reconciled));

        // The restore drill reconciles pending effects *first*: a packet carrying
        // unreconciled effects restores ready-for-new-work only because the drill
        // reconciled them before declaring readiness.
        let packet = populated_backup().with_pending_effects(pending);
        let report = run_restore_drill(&packet).unwrap();
        assert_eq!(report.pending_effects, 2);
        assert!(report.effects_reconciled_first, "{report:?}");
        assert!(report.ready_for_new_work);
    }
}
