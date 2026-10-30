//! The `entelechy drill` command: operational disaster-recovery and audit drills.
//!
//! Three drills, selected with `--kind` (default: all):
//!
//! - **restore** (PRD §17.3, DR-1): builds a realistic slice of durable state —
//!   content-addressed artifacts across two security domains, the append-only
//!   metadata/event log, a lineage graph and an erasure tombstone — backs it up to
//!   serialized bytes, restores it through the self-verifying
//!   [`entelechy_artifacts::run_restore_drill`], prints the resulting
//!   [`entelechy_artifacts::RestoreReport`], and proves the drill is fail-closed by
//!   tampering a copy of the backup and showing the restore is *rejected*.
//! - **rollback** (PRD §14.6, RB-1/RB-2): runs [`entelechy_release::rollback_drill`]
//!   — the real promote → deploy → trigger → rollback → revoke sequence — and
//!   reports that a production release without a tested rollback target is refused
//!   (RB-2), that stop authority is separate from promote authority and declared
//!   triggers fire automatic rollback (RB-1), and that a rollback recovers the prior
//!   known-good release.
//! - **audit** (PRD §17.2, AU-1/AU-2/AU-3): runs [`entelechy_identity::audit_drill`]
//!   — an authenticated privileged operation whose credential expiry fails closed
//!   and whose authentication never implies authorization (AU-1), a signing-key
//!   lifecycle that rotates and revokes without mutating the content-addressed
//!   artifact (AU-2), an uncertain clock that fails closed (AU-3), and a
//!   hash-chained, externally-anchored audit log that detects a full rewrite (AU-1).
//!
//! `--out <dir>` persists each drill's artifacts into that directory so the
//! verification record is auditable.

use entelechy_artifacts::{
    run_restore_drill, Artifact, ArtifactHeader, BackupPacket, BlobStore, ErasureLedger,
    InMemoryMetadataStore, InMemoryStore, LineageEdge, LineageKind, MetadataStore, RestoreError,
    SchemaVersion, DEFAULT_TENANT,
};
use entelechy_identity::{audit_drill, AuditDrillReport};
use entelechy_release::{rollback_drill, RollbackDrillReport};

/// Which disaster-recovery drill(s) to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum DrillKind {
    /// The DR-1 backup → restore → verify drill (PRD §17.3).
    Restore,
    /// The RB-1/RB-2 rollback drill (PRD §14.6).
    Rollback,
    /// The AU-1/AU-2/AU-3 audit/attestation drill (PRD §17.2).
    Audit,
    /// Every drill.
    All,
}

/// Run the selected disaster-recovery/audit drill(s). With `out`, persists each
/// drill's artifacts into that directory.
pub fn run(kind: DrillKind, out: Option<&str>) -> anyhow::Result<()> {
    if matches!(kind, DrillKind::Restore | DrillKind::All) {
        run_restore(out)?;
    }
    if matches!(kind, DrillKind::All) {
        println!();
    }
    if matches!(kind, DrillKind::Rollback | DrillKind::All) {
        run_rollback(out)?;
    }
    if matches!(kind, DrillKind::All) {
        println!();
    }
    if matches!(kind, DrillKind::Audit | DrillKind::All) {
        run_audit(out)?;
    }
    Ok(())
}

/// Run the DR-1 backup/restore drill (PRD §17.3). With `out`, persists the backup
/// packet and the restore report into that directory.
fn run_restore(out: Option<&str>) -> anyhow::Result<()> {
    // 1. Build a realistic slice of durable state across two security domains.
    let packet = populate_durable_state()?;
    let tenants = distinct_tenants(&packet);
    let backup_bytes = packet.to_bytes()?;
    println!(
        "Disaster-recovery drill (PRD §17.3, DR-1) — backing up durable state.\n\
         Backed up {} content-addressed object(s) across {} security domain(s), \
         {} metadata event(s), {} lineage edge(s) and {} erasure tombstone(s) \
         into {} serialized byte(s).\n",
        packet.objects.len(),
        tenants.len(),
        packet.events.len(),
        packet.lineage.len(),
        packet.erasure.len(),
        backup_bytes.len(),
    );

    // 2. Restore through the self-verifying drill and report each DR-1 invariant.
    let report = run_restore_drill(&packet)?;
    println!("Restore drill — DR-1 invariants:");
    println!(
        "  hashes preserved:          {}/{} object(s) re-hashed to their address",
        report.hashes_verified, report.objects_restored
    );
    println!(
        "  authorization preserved:   {} (each object readable only by its owning tenant — PRD §17.2)",
        yes_no(report.authorization_preserved)
    );
    println!(
        "  lineage preserved:         {} edge(s) + {} event(s) round-tripped unchanged",
        report.lineage_edges, report.events_restored
    );
    println!(
        "  erasures preserved:        {} tombstone(s) survived the restore (DR-2 — crypto-shredded data never reappears)",
        report.erasures_preserved
    );
    println!(
        "\n  => DR-1 {}.",
        if report.ok {
            "SATISFIED — backup and restore preserve hashes, lineage and authorization metadata"
        } else {
            "VIOLATED — an invariant did not hold (see above)"
        }
    );
    if !report.ok {
        anyhow::bail!("DR-1 restore drill failed: an invariant did not hold");
    }

    // 3. Prove the drill is fail-closed: a corrupt backup is rejected, not loaded.
    let mut tampered = packet.clone();
    if let Some(obj) = tampered.objects.first_mut() {
        obj.bytes.push(b'!'); // one flipped byte breaks the content address
    }
    match run_restore_drill(&tampered) {
        Err(RestoreError::IntegrityFailure(id)) => println!(
            "\nFail-closed check: a tampered backup is REJECTED — object {id} no longer matches its hash (PRD §17.3)."
        ),
        other => anyhow::bail!("expected a tampered backup to be rejected, got {other:?}"),
    }

    // 4. Persist the backup packet and the verification record.
    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        let base = std::path::Path::new(dir);
        let packet_path = base.join("backup-packet.json");
        let report_path = base.join("restore-report.json");
        std::fs::write(&packet_path, &backup_bytes)?;
        std::fs::write(&report_path, serde_json::to_string_pretty(&report)?)?;
        println!(
            "\nPersisted: {} ({} bytes) and {}.",
            packet_path.display(),
            backup_bytes.len(),
            report_path.display()
        );
    }
    Ok(())
}

/// Run the RB-1/RB-2 rollback drill (PRD §14.6). With `out`, persists the report.
fn run_rollback(out: Option<&str>) -> anyhow::Result<()> {
    let report: RollbackDrillReport = rollback_drill();
    let triggers: Vec<String> = report
        .declared_triggers
        .iter()
        .map(|t| format!("{t:?}"))
        .collect();

    println!(
        "Rollback drill (PRD §14.6, RB-1/RB-2) — promote → deploy → trigger → rollback → revoke."
    );
    println!("Rollback readiness (RB-2):");
    println!(
        "  production promotion gated: {} (an L4/L5 release with no rollback target and no rationale is refused)",
        yes_no(report.rollback_ready_enforced)
    );
    println!(
        "  no-rollback rationale path: {} (a documented, approved rationale also satisfies readiness)",
        yes_no(report.rationale_satisfies_rb2)
    );
    println!("Rollback authority + triggers (RB-1):");
    println!(
        "  stop ≠ promote authority:   {} (an on-call stopper cannot deploy; a releaser cannot roll back or revoke)",
        yes_no(report.authority_separation_enforced)
    );
    println!(
        "  declared auto-triggers:     [{}]; declared fires: {}, undeclared ignored: {}",
        triggers.join(", "),
        yes_no(report.declared_trigger_fired),
        yes_no(report.undeclared_trigger_ignored)
    );
    println!("Recovery:");
    println!(
        "  rolled back to:             {} (prior known-good release restored as active)",
        report.recovered_to
    );
    println!(
        "  recovery verified:          {}",
        yes_no(report.recovery_ok)
    );
    println!(
        "  emergency revocation:       {} (revoking the active release serves nothing — fail-safe stop)",
        yes_no(report.revocation_ok)
    );
    println!(
        "\n  => RB-1/RB-2 {}.",
        if report.ok {
            "SATISFIED — declared rollback triggers, separated stop authority, and a tested rollback target recover a bad rollout"
        } else {
            "VIOLATED — an invariant did not hold (see above)"
        }
    );
    if !report.ok {
        anyhow::bail!("RB rollback drill failed: an invariant did not hold");
    }

    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        let report_path = std::path::Path::new(dir).join("rollback-report.json");
        std::fs::write(&report_path, serde_json::to_string_pretty(&report)?)?;
        println!("\nPersisted: {}.", report_path.display());
    }
    Ok(())
}

/// Run the AU-1/AU-2/AU-3 audit/attestation drill (PRD §17.2). With `out`, persists
/// the report.
fn run_audit(out: Option<&str>) -> anyhow::Result<()> {
    let report: AuditDrillReport = audit_drill();

    println!(
        "Audit/attestation drill (PRD §17.2, AU-1/AU-2/AU-3) — authenticated identity, key lifecycle, tamper-evident log."
    );
    println!("Authenticated privileged operation (AU-1):");
    println!(
        "  authenticated principal:    {} (a privileged operation carries an authenticated identity)",
        report.authenticated_principal
    );
    println!(
        "  authn ≠ authz:              {} (separation of duties still refuses a self-approval)",
        yes_no(report.authentication_requires_authorization)
    );
    println!(
        "  credential expiry closes:   {} (an expired credential is refused)",
        yes_no(report.expiry_fails_closed)
    );
    println!("Tamper-evident audit log (AU-1):");
    println!(
        "  privileged actions logged:  {} entries in the hash chain",
        report.audit_entries
    );
    println!(
        "  hash chain verifies:        {}",
        yes_no(report.audit_chain_valid)
    );
    println!(
        "  external anchor holds:      {} (an externally-held anchor is still in the chain's history)",
        yes_no(report.audit_anchor_holds)
    );
    println!(
        "  full rewrite detected:      {} (a forged, internally-consistent log no longer matches the anchor)",
        yes_no(report.tamper_detected)
    );
    println!("Key lifecycle without plaintext mutation (AU-2):");
    println!(
        "  original key valid first:   {} (the pre-lifecycle approval was valid)",
        yes_no(report.approval_valid_before_revocation)
    );
    println!(
        "  rotated key signs writes:   {} (a new key signs and validates new writes)",
        yes_no(report.rotated_key_signs_new_writes)
    );
    println!(
        "  revoked key cannot sign:    {} (a revoked key never signs a new write)",
        yes_no(report.revoked_key_cannot_sign)
    );
    println!(
        "  revoked approval closes:    {} (the revoked key's existing approval fails closed)",
        yes_no(report.revoked_key_approval_fails_closed)
    );
    println!(
        "  artifact address stable:    {} (rotation/revocation never mutate the signed artifact)",
        yes_no(report.artifact_address_stable)
    );
    println!("Uncertain clock (AU-3):");
    println!(
        "  time uncertainty closes:    {} (an uncertain clock fails closed rather than defaulting to valid)",
        yes_no(report.uncertain_time_fails_closed)
    );
    println!(
        "\n  => AU-1/AU-2/AU-3 {}.",
        if report.ok {
            "SATISFIED — authenticated identity with fail-closed expiry, a rotatable/revocable key lifecycle that never mutates artifacts, and a tamper-evident, externally-anchored audit log"
        } else {
            "VIOLATED — an invariant did not hold (see above)"
        }
    );
    if !report.ok {
        anyhow::bail!("AU audit drill failed: an invariant did not hold");
    }

    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        let report_path = std::path::Path::new(dir).join("audit-report.json");
        std::fs::write(&report_path, serde_json::to_string_pretty(&report)?)?;
        println!("\nPersisted: {}.", report_path.display());
    }
    Ok(())
}

/// Count the distinct owning tenants across the backed-up objects.
fn distinct_tenants(packet: &BackupPacket) -> std::collections::BTreeSet<&str> {
    packet.objects.iter().map(|o| o.tenant.as_str()).collect()
}

/// Build a realistic slice of durable state across two security domains, with a
/// lineage graph, an append-only event log and an erasure tombstone, and back it
/// up. The artifacts are real canonical [`Artifact`]s so their content addresses
/// are exactly what the running system would store.
fn populate_durable_state() -> anyhow::Result<BackupPacket> {
    let mut store = InMemoryStore::new();
    let mut meta = InMemoryMetadataStore::new();

    // --- Domain "local": a signed objective, the design derived from it, a
    //     semantics-preserving migration of that design, and a promoted release.
    let goalspec = artifact(
        "entelechy.goalspec",
        serde_json::json!({ "goal": "resolve tier-1 tickets" }),
    );
    let design_v1 = artifact(
        "entelechy.design",
        serde_json::json!({ "design": "triage-v1" }),
    );
    let design_v2 = artifact(
        "entelechy.design",
        serde_json::json!({ "design": "triage-v2" }),
    );
    let release = artifact(
        "entelechy.release",
        serde_json::json!({ "release": "L2", "design": "triage-v2" }),
    );

    let id_goal = store.put(DEFAULT_TENANT, goalspec.canonical_bytes()?);
    let id_d1 = store.put(DEFAULT_TENANT, design_v1.canonical_bytes()?);
    let id_d2 = store.put(DEFAULT_TENANT, design_v2.canonical_bytes()?);
    let id_rel = store.put(DEFAULT_TENANT, release.canonical_bytes()?);

    meta.append_event(
        DEFAULT_TENANT,
        "goalspec.signed",
        Some(id_goal.clone()),
        serde_json::json!({}),
    );
    meta.append_event(
        DEFAULT_TENANT,
        "design.synthesized",
        Some(id_d1.clone()),
        serde_json::json!({}),
    );
    meta.append_event(
        DEFAULT_TENANT,
        "design.migrated",
        Some(id_d2.clone()),
        serde_json::json!({}),
    );
    meta.append_event(
        DEFAULT_TENANT,
        "release.promoted",
        Some(id_rel.clone()),
        serde_json::json!({ "level": "L2" }),
    );

    // --- Domain "tenant-b": a separate design in an isolated security domain,
    //     with a subject later crypto-shredded (erasure).
    let other_design = artifact(
        "entelechy.design",
        serde_json::json!({ "design": "tenant-b-triage" }),
    );
    let id_b = store.put("tenant-b", other_design.canonical_bytes()?);
    meta.append_event(
        "tenant-b",
        "design.synthesized",
        Some(id_b),
        serde_json::json!({}),
    );

    let lineage = vec![
        LineageEdge {
            from: id_goal,
            to: id_d1.clone(),
            kind: LineageKind::DerivedFrom,
        },
        LineageEdge {
            from: id_d1,
            to: id_d2.clone(),
            kind: LineageKind::MigrationOf {
                semantics_preserving: true,
            },
        },
        LineageEdge {
            from: id_d2,
            to: id_rel,
            kind: LineageKind::Supersedes,
        },
    ];

    let mut erasure = ErasureLedger::new();
    erasure.erase("tenant-b/subject-42");

    Ok(BackupPacket::backup(
        &store,
        &meta.export(),
        &lineage,
        &erasure,
    ))
}

/// Build a canonical artifact for the given schema with the local default tenant.
fn artifact(schema_id: &str, payload: serde_json::Value) -> Artifact<serde_json::Value> {
    Artifact::new(
        ArtifactHeader::new(schema_id, SchemaVersion::new(1, 0, 0), "0.0.0"),
        payload,
    )
}

/// Render a boolean DR-1 invariant as `yes`/`NO`.
fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "NO"
    }
}
