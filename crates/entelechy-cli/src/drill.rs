//! The `entelechy drill` command: a backup → restore → verify disaster-recovery
//! drill (PRD §17.3, DR-1).
//!
//! DR-1 requires that "backup and restore preserve hashes, lineage and
//! authorization metadata". This command builds a realistic slice of durable
//! state — content-addressed artifacts across two security domains, the
//! append-only metadata/event log, a lineage graph and an erasure tombstone —
//! backs it up to serialized bytes, restores it through the self-verifying
//! [`entelechy_artifacts::run_restore_drill`], and prints the resulting
//! [`entelechy_artifacts::RestoreReport`]. It then proves the drill is
//! fail-closed by tampering a copy of the backup and showing the restore is
//! *rejected*, never silently loaded. `--out` persists the backup packet and the
//! report so the verification record is auditable.

use entelechy_artifacts::{
    run_restore_drill, Artifact, ArtifactHeader, BackupPacket, BlobStore, ErasureLedger,
    InMemoryMetadataStore, InMemoryStore, LineageEdge, LineageKind, MetadataStore, RestoreError,
    SchemaVersion, DEFAULT_TENANT,
};

/// Run the DR-1 backup/restore drill (PRD §17.3). With `out`, persists the backup
/// packet and the restore report into that directory.
pub fn run(out: Option<&str>) -> anyhow::Result<()> {
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
