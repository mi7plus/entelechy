//! The `entelechy drill` command: operational disaster-recovery, audit and SLO
//! drills.
//!
//! The drills, selected with `--kind` (default: all):
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
//! - **slo** (PRD §18.1, SL-1): runs [`entelechy_release::slo_drill`] — an L5 SLO
//!   that must state its window, population, statistic/error budget and exclusions
//!   (fail closed on any omission), and the proof that *correctness gates sit outside
//!   the error budget*: a correctness-gate failure blocks with the budget fully
//!   intact, a budget breach with clean correctness is only an operational freeze,
//!   and the real L5 promotion gate takes no budget input — only a recorded override
//!   admits a failed correctness gate.
//! - **drift** (PRD 5.10, PV-3): runs [`entelechy_domain::drift_drill`] — builds
//!   real executed evidence under a calibrated model revision and a silently-drifted
//!   one and proves that a drift regressing behavior beyond a declared tolerance
//!   blocks the *automatic* promotion path, while a within-tolerance drift and a
//!   recorded human override do not (PV-3 blocks automatic promotion, not every
//!   promotion).
//! - **degraded** (PRD 17.4, OL-2): runs [`entelechy_ops::degraded_mode_drill`] — a
//!   fallback is authorized on its own capabilities and confidentiality tier (never
//!   inherited from the primary), entering degraded mode is journaled with a reason,
//!   and observations gathered while degraded are excluded from the registered
//!   evidence a release rests on.
//! - **rollout** (PRD §14.2, §21.4 — a Phase-4 *roadmap feature*, not a §17.6
//!   registry requirement): runs [`entelechy_release::rollout_drill`] — the staged
//!   shadow (L3) and canary (L4) rollout path. Shadow requires effect substitution
//!   (a consequential effect with no dry-run makes a design shadow-ineligible) and
//!   commits none of the candidate's effects; the canary routes a *bounded* fraction
//!   of traffic to the candidate and turns the slice's error budget, metric drift and
//!   correctness into a promote/hold/rollback verdict. A healthy canary promotes only
//!   through the *real* L4→L5 gate (never a bypass); a regressing or
//!   correctness-failed canary rolls back to the prior known-good release.
//! - **enforcement** (PRD §8, §5.4 — the WP1 runtime-correctness gate): runs
//!   [`entelechy_runtime::enforcement_drill`] — adversarial programs on the real
//!   interpreter proving a *discovered* architecture cannot launder information-flow
//!   labels past a policy gate (WS1), exercise authority a `Delegate` removed (WS2),
//!   bypass authorization on the counterfactual path (WS3), replay from the wrong
//!   input (WS4), or complete with an effect whose commit outcome is unknown (WS5).
//! - **shadow** (PRD §14.2 L3 — a Phase-4 *roadmap feature*, not a §17.6 registry
//!   requirement): runs [`entelechy_runtime::shadow_drill`] — runtime effect
//!   substitution. A consequential tool effect a normal run dispatches is intercepted
//!   in shadow (the tool gateway is never called) and recorded instead, so a shadow
//!   run's journal carries no committed effect; a spy gateway proves zero dispatches
//!   while the same program run normally dispatches once, and shadow still enforces
//!   the authority gate — effect substitution is enforced by the engine, not trusted
//!   from a capability's `dry_run_supported` metadata.
//!
//! `--out <dir>` persists each drill's artifacts into that directory so the
//! verification record is auditable.

use entelechy_artifacts::{
    run_restore_drill, Artifact, ArtifactHeader, BackupPacket, BlobStore, ErasureLedger,
    InMemoryMetadataStore, InMemoryStore, LineageEdge, LineageKind, MetadataStore, PendingEffect,
    RestoreError, SchemaVersion, DEFAULT_TENANT,
};
use entelechy_domain::{drift_drill, DriftDrillReport};
use entelechy_effects::OperationKey;
use entelechy_identity::{audit_drill, AuditDrillReport};
use entelechy_ops::{degraded_mode_drill, DegradedModeDrillReport};
use entelechy_release::{
    rollback_drill, rollout_drill, slo_drill, RollbackDrillReport, RolloutDrillReport,
    SloDrillReport,
};
use entelechy_runtime::{
    enforcement_drill, shadow_drill, EnforcementDrillReport, ShadowDrillReport,
};

/// Which disaster-recovery drill(s) to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum DrillKind {
    /// The DR-1 backup → restore → verify drill (PRD §17.3).
    Restore,
    /// The RB-1/RB-2 rollback drill (PRD §14.6).
    Rollback,
    /// The AU-1/AU-2/AU-3 audit/attestation drill (PRD §17.2).
    Audit,
    /// The SL-1 SLO / error-budget drill (PRD §18.1).
    Slo,
    /// The PV-3 drift / automatic-promotion drill (PRD §5.10).
    Drift,
    /// The OL-2 degraded-mode / fallback drill (PRD §17.4).
    Degraded,
    /// The staged shadow/canary rollout drill (PRD §14.2, §21.4).
    Rollout,
    /// The WP1 runtime-correctness enforcement drill (PRD §8, §5.4) — information
    /// flow, authority narrowing, mode parity, replay and write-ahead intent.
    Enforcement,
    /// The WP2 shadow write-interception drill (PRD §14.2 L3) — runtime effect
    /// substitution: shadow commits no consequential effect, enforced by the engine
    /// rather than trusted from capability metadata.
    Shadow,
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
    if matches!(kind, DrillKind::All) {
        println!();
    }
    if matches!(kind, DrillKind::Slo | DrillKind::All) {
        run_slo(out)?;
    }
    if matches!(kind, DrillKind::All) {
        println!();
    }
    if matches!(kind, DrillKind::Drift | DrillKind::All) {
        run_drift(out)?;
    }
    if matches!(kind, DrillKind::All) {
        println!();
    }
    if matches!(kind, DrillKind::Degraded | DrillKind::All) {
        run_degraded(out)?;
    }
    if matches!(kind, DrillKind::All) {
        println!();
    }
    if matches!(kind, DrillKind::Rollout | DrillKind::All) {
        run_rollout(out)?;
    }
    if matches!(kind, DrillKind::All) {
        println!();
    }
    if matches!(kind, DrillKind::Enforcement | DrillKind::All) {
        run_enforcement(out)?;
    }
    if matches!(kind, DrillKind::All) {
        println!();
    }
    if matches!(kind, DrillKind::Shadow | DrillKind::All) {
        run_shadow_enforcement(out)?;
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
        "Disaster-recovery drill (PRD §17.3, DR-1/DR-2) — backing up durable state.\n\
         Backed up {} content-addressed object(s) across {} security domain(s), \
         {} metadata event(s), {} lineage edge(s), {} erasure tombstone(s), \
         {} revocation(s) and {} in-flight effect(s) pending reconciliation \
         into {} serialized byte(s).\n",
        packet.objects.len(),
        tenants.len(),
        packet.events.len(),
        packet.lineage.len(),
        packet.erasure.len(),
        packet.revocations.len(),
        packet.pending_effects.len(),
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
        "  revocations preserved:     {} revocation(s) survived the restore (DR-2 — a revoked release/key is never resurrected)",
        report.revocations_preserved
    );
    println!(
        "  effects reconciled first:  {} ({} in-flight effect(s) with an unknown commit reconciled before new work — DR-2)",
        yes_no(report.effects_reconciled_first),
        report.pending_effects
    );
    println!(
        "  ready for new work:        {} (new work resumes only once every DR-1/DR-2 invariant holds)",
        yes_no(report.ready_for_new_work)
    );
    println!(
        "\n  => DR-1/DR-2 {}.",
        if report.ok {
            "SATISFIED — backup and restore preserve hashes, lineage and authorization; erasures and revocations survive, and in-flight effects are reconciled before new work"
        } else {
            "VIOLATED — an invariant did not hold (see above)"
        }
    );
    if !report.ok {
        anyhow::bail!("DR-1/DR-2 restore drill failed: an invariant did not hold");
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

/// Run the SL-1 SLO / error-budget drill (PRD §18.1). With `out`, persists the report.
fn run_slo(out: Option<&str>) -> anyhow::Result<()> {
    let report: SloDrillReport = slo_drill();
    let defects: Vec<String> = report
        .detected_defects
        .iter()
        .map(|d| format!("{d:?}"))
        .collect();

    println!(
        "SLO / error-budget drill (PRD §18.1, SL-1) — L5 SLO well-formedness; correctness sits outside the error budget."
    );
    println!("L5 SLO declaration (SL-1):");
    println!(
        "  well-formed SLO accepted:   {} (states window, population, statistic/error budget and exclusions)",
        yes_no(report.wellformed_slo_accepted)
    );
    println!(
        "  incomplete SLO rejected:    {} (an L5 SLO missing any required field fails closed)",
        yes_no(report.incomplete_slo_rejected)
    );
    println!("    defects detected:         [{}]", defects.join(", "));
    println!(
        "  error budget derived:       {} (budget = 1 - target; spend tracked over observed events)",
        yes_no(report.error_budget_derived)
    );
    println!("Correctness outside the error budget (SL-1):");
    println!(
        "  full budget never excuses:  {} (a correctness-gate failure blocks with the budget fully intact)",
        yes_no(report.correctness_blocks_despite_full_budget)
    );
    println!(
        "  budget breach is ops-only:  {} (an exhausted budget with clean correctness is a freeze, not a correctness block)",
        yes_no(report.budget_breach_is_operational)
    );
    println!(
        "  correctness escape gated:   {} (the L5 gate refuses a failed correctness gate and takes no budget input; only a recorded override admits it)",
        yes_no(report.correctness_escape_is_explicit_override)
    );
    println!(
        "\n  => SL-1 {}.",
        if report.ok {
            "SATISFIED — every L5 SLO states window/population/statistic/exclusions, and correctness gates sit outside the error budget"
        } else {
            "VIOLATED — an invariant did not hold (see above)"
        }
    );
    if !report.ok {
        anyhow::bail!("SL SLO drill failed: an invariant did not hold");
    }

    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        let report_path = std::path::Path::new(dir).join("slo-report.json");
        std::fs::write(&report_path, serde_json::to_string_pretty(&report)?)?;
        println!("\nPersisted: {}.", report_path.display());
    }
    Ok(())
}

/// Run the PV-3 drift drill (PRD §5.10). With `out`, persists the report.
fn run_drift(out: Option<&str>) -> anyhow::Result<()> {
    let report: DriftDrillReport = drift_drill();

    println!(
        "Drift / automatic-promotion drill (PRD §5.10, PV-3) — drift beyond tolerance blocks automatic promotion."
    );
    println!("Observed drift (real executed evidence):");
    println!(
        "  material drift observed:    {} (a silently-drifted model revision opened a new evidence epoch)",
        yes_no(report.drift_detected)
    );
    println!(
        "  behavioral regression:      {:.1}pp (tolerance {:.1}pp)",
        report.regression_pp, report.tolerance_pp
    );
    println!("Automatic promotion gate (PV-3):");
    println!(
        "  beyond tolerance blocks:    {} (a drift that regresses beyond tolerance blocks the automatic path)",
        yes_no(report.beyond_tolerance_blocks_auto)
    );
    println!(
        "  within tolerance allows:    {} (a material drift that does not regress behavior still auto-promotes)",
        yes_no(report.within_tolerance_allows_auto)
    );
    println!(
        "  no drift allows:            {} (a single-epoch, no-drift history auto-promotes)",
        yes_no(report.no_drift_allows_auto)
    );
    println!(
        "  human override still goes:  {} (a recorded override promotes a drift-blocked release; the block stands without one)",
        yes_no(report.human_override_still_promotes)
    );
    println!(
        "\n  => PV-3 {}.",
        if report.ok {
            "SATISFIED — drift beyond tolerance blocks automatic promotion; within-tolerance drift and a recorded human override do not"
        } else {
            "VIOLATED — an invariant did not hold (see above)"
        }
    );
    if !report.ok {
        anyhow::bail!("PV-3 drift drill failed: an invariant did not hold");
    }

    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        let report_path = std::path::Path::new(dir).join("drift-report.json");
        std::fs::write(&report_path, serde_json::to_string_pretty(&report)?)?;
        println!("\nPersisted: {}.", report_path.display());
    }
    Ok(())
}

/// Run the OL-2 degraded-mode drill (PRD §17.4). With `out`, persists the report.
fn run_degraded(out: Option<&str>) -> anyhow::Result<()> {
    let report: DegradedModeDrillReport = degraded_mode_drill();

    println!(
        "Degraded-mode / fallback drill (PRD §17.4, OL-2) — fallbacks independently authorized; degraded evidence excluded."
    );
    println!("Independently-authorized fallbacks (OL-2):");
    println!(
        "  independent fallback admitted: {} (a fallback holding the required capabilities at the required tier runs)",
        yes_no(report.fallback_authorized_when_independent)
    );
    println!(
        "  missing capability refused:    {} (a fallback does not inherit a capability from the primary)",
        yes_no(report.fallback_refused_when_missing_capability)
    );
    println!(
        "  weaker confidentiality refused:{} (a fallback that would weaken confidentiality is refused)",
        yes_no(report.fallback_refused_when_weaker_confidentiality)
    );
    println!("Journaled degraded period (OL-2):");
    println!(
        "  entering degraded journaled:   {} (the period of reduced guarantees is recorded with a reason)",
        yes_no(report.degraded_mode_journaled)
    );
    println!("Evidence firewall (OL-2):");
    println!(
        "  degraded evidence excluded:    {} ({} degraded observation(s) excluded; {} normal observation(s) registered)",
        yes_no(report.degraded_evidence_excluded),
        report.excluded_count,
        report.registered_count
    );
    println!(
        "\n  => OL-2 {}.",
        if report.ok {
            "SATISFIED — fallbacks are independently authorized, the degraded period is journaled, and degraded-mode observations never back a release"
        } else {
            "VIOLATED — an invariant did not hold (see above)"
        }
    );
    if !report.ok {
        anyhow::bail!("OL-2 degraded-mode drill failed: an invariant did not hold");
    }

    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        let report_path = std::path::Path::new(dir).join("degraded-report.json");
        std::fs::write(&report_path, serde_json::to_string_pretty(&report)?)?;
        println!("\nPersisted: {}.", report_path.display());
    }
    Ok(())
}

/// Run the staged shadow/canary rollout drill (PRD §14.2, §21.4). With `out`,
/// persists the report.
fn run_rollout(out: Option<&str>) -> anyhow::Result<()> {
    let report: RolloutDrillReport = rollout_drill();

    println!(
        "Shadow/canary rollout drill (PRD §14.2, §21.4) — staged L3 shadow → L4 canary → L5 gate.\n\
         (A Phase-4 roadmap feature, not a §17.6 registry requirement — it does not move the TR-1 gate.)"
    );
    println!("Shadow stage (L3 — production inputs without consequential effects):");
    println!(
        "  effect substitution required: {} (a consequential effect with no dry-run makes a design shadow-ineligible)",
        yes_no(report.shadow_requires_dry_run)
    );
    println!(
        "  candidate commits nothing:    {} (every candidate consequential effect is substituted; only the baseline serves production)",
        yes_no(report.shadow_commits_no_candidate_effects)
    );
    println!(
        "  parity proceeds to canary:    {} (high agreement and no new errors clears the candidate for L4)",
        yes_no(report.shadow_parity_proceeds)
    );
    println!(
        "  new errors hold:              {} (a candidate that newly errors does not proceed)",
        yes_no(report.shadow_divergence_holds)
    );
    println!("Canary stage (L4 — bounded production authority and monitored rollout):");
    println!(
        "  bounded traffic fraction:     {} (only a small, deterministic slice is routed to the candidate)",
        yes_no(report.canary_routes_bounded_fraction)
    );
    println!(
        "  healthy canary promotes:      {} (a healthy slice promotes through the real L4→L5 gate)",
        yes_no(report.healthy_canary_promotes_through_gate)
    );
    println!(
        "  canary never bypasses gate:   {} (the same healthy canary is refused without valid approvals)",
        yes_no(report.healthy_canary_still_gated)
    );
    println!(
        "  budget breach rolls back:     {} (an exhausted canary error budget rolls back to the known-good release)",
        yes_no(report.regressing_canary_rolls_back)
    );
    println!(
        "  correctness failure rolls back: {} (a failed correctness gate rolls back regardless of the operational signals)",
        yes_no(report.correctness_failure_rolls_back)
    );
    println!(
        "\n  => staged rollout {}.",
        if report.ok {
            "SATISFIED — shadow substitutes all candidate effects, the canary is bounded, a healthy slice promotes only through the real gate, and a regression rolls back"
        } else {
            "VIOLATED — an invariant did not hold (see above)"
        }
    );
    if !report.ok {
        anyhow::bail!("staged rollout drill failed: an invariant did not hold");
    }

    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        let report_path = std::path::Path::new(dir).join("rollout-report.json");
        std::fs::write(&report_path, serde_json::to_string_pretty(&report)?)?;
        println!("\nPersisted: {}.", report_path.display());
    }
    Ok(())
}

/// Run the WP1 runtime-correctness enforcement drill (PRD §8, §5.4). With `out`,
/// persists the report.
fn run_enforcement(out: Option<&str>) -> anyhow::Result<()> {
    let report: EnforcementDrillReport = enforcement_drill();

    println!(
        "WP1 runtime-correctness enforcement drill (PRD §8, §5.4, §7.5, §8.5) — a \
         discovered architecture cannot be made to violate the runtime's constraints."
    );
    println!("Information flow (WS1 — IR-I9 / PRD §7.5):");
    println!(
        "  Map cannot launder taint:    {} (a tainted collection cannot reach a consequential egress through a Map)",
        yes_no(report.map_cannot_launder_taint)
    );
    println!(
        "  trusted fan-out not tainted: {} (the same egress is authorized for a trusted collection — no over-tainting)",
        yes_no(report.trusted_fanout_not_overtainted)
    );
    println!("Authority narrowing (WS2 — A2 / IR-I6 / PRD §5.4):");
    println!(
        "  Delegate narrows authority:  {} (a capability a Delegate drops is denied inside its subtree)",
        yes_no(report.delegate_narrows_authority)
    );
    println!(
        "  widening Delegate halts:     {} (a Delegate that widens the active envelope halts at runtime)",
        yes_no(report.delegate_widening_halts)
    );
    println!("Execution-mode parity (WS3 — PRD §8.3 / §10.2):");
    println!(
        "  counterfactual authorized:   {} (a live counterfactual effect passes the same authorization gate as Execute)",
        yes_no(report.counterfactual_effect_authorized)
    );
    println!("Deterministic replay (WS4 — RK-2):");
    println!(
        "  replay from recorded input:  {} (replay reconstructs from the recorded initial input, not an empty object)",
        yes_no(report.replay_uses_recorded_input)
    );
    println!(
        "  input tampering detected:    {} (replaying a different input than recorded is rejected)",
        yes_no(report.replay_detects_input_tampering)
    );
    println!("Write-ahead intent + reconciliation (WS5 — PRD §7.6 / §8.5):");
    println!(
        "  unacknowledged reconciles:   {} (a lost acknowledgement leaves a durable intent and forces reconciliation)",
        yes_no(report.unacknowledged_effect_reconciles)
    );
    println!(
        "  acknowledged commits:        {} (an acknowledged effect resolves its intent to Committed, no dangling intent)",
        yes_no(report.acknowledged_effect_commits)
    );
    println!(
        "\n  => WP1 runtime correctness {}.",
        if report.ok {
            "SATISFIED — the runtime enforces information-flow integrity, authority narrowing, execution-mode parity, deterministic replay and write-ahead reconciliation against any discovered architecture"
        } else {
            "VIOLATED — an invariant did not hold (see above)"
        }
    );
    if !report.ok {
        anyhow::bail!("WP1 enforcement drill failed: an invariant did not hold");
    }

    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        let report_path = std::path::Path::new(dir).join("enforcement-report.json");
        std::fs::write(&report_path, serde_json::to_string_pretty(&report)?)?;
        println!("\nPersisted: {}.", report_path.display());
    }
    Ok(())
}

/// Run the WP2 shadow write-interception drill (PRD §14.2 L3). With `out`, persists
/// the report.
fn run_shadow_enforcement(out: Option<&str>) -> anyhow::Result<()> {
    let report: ShadowDrillReport = shadow_drill();

    println!(
        "Shadow write-interception drill (PRD §14.2 L3) — effect substitution is a \
         runtime guarantee, not trusted metadata.\n\
         (A Phase-4 roadmap feature, not a §17.6 registry requirement — it does not move the TR-1 gate.)"
    );
    println!("Runtime effect substitution (L3):");
    println!(
        "  shadow dispatches nothing:    {} (a consequential tool effect is intercepted; the tool gateway is never called)",
        yes_no(report.shadow_does_not_dispatch)
    );
    println!(
        "  execute dispatches (control): {} (the same program run normally dispatches the effect exactly once)",
        yes_no(report.execute_dispatches)
    );
    println!(
        "  journal records no effect:    {} (a shadow run's journal carries zero committed tool effects — the structural proof)",
        yes_no(report.shadow_journal_has_no_effect)
    );
    println!(
        "  substitution recorded:        {} (what would have run is logged out of band for evidence)",
        yes_no(report.shadow_records_substitution)
    );
    println!(
        "  authority still enforced:     {} (a delegate-narrowed capability is denied in shadow too — not a privileged bypass)",
        yes_no(report.shadow_still_enforces_authority)
    );
    println!(
        "\n  => shadow write-interception {}.",
        if report.ok {
            "SATISFIED — the runtime substitutes every consequential tool effect in shadow (the gateway is never called) while still enforcing authority, so shadow commits nothing regardless of capability metadata"
        } else {
            "VIOLATED — an invariant did not hold (see above)"
        }
    );
    if !report.ok {
        anyhow::bail!("shadow write-interception drill failed: an invariant did not hold");
    }

    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        let report_path = std::path::Path::new(dir).join("shadow-report.json");
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

    // DR-2 state carried through the backup: a release revoked before the backup
    // must stay revoked across a restore, and an in-flight effect whose commit
    // outcome was unknown at backup time must be reconciled before the restored
    // state resumes new work. The pending effect's operation key is the real key the
    // effects write-ahead log records for that effect (PRD 7.6), tying the drill's
    // reconcile-first display to the actual reconciliation model.
    let pending_key = OperationKey::new("release-run-7", "root/deploy").as_string();
    Ok(
        BackupPacket::backup(&store, &meta.export(), &lineage, &erasure)
            .with_revocations(["rel-L1-withdrawn".to_string()])
            .with_pending_effects([PendingEffect::unknown(pending_key)]),
    )
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
