//! `entelechy assure` and `entelechy release`: run the assurance compiler and
//! drive maturity promotion through the release gate with hash-bound approvals.
//!
//! Ties together assurance (14.1), release (14.2-14.6) and identity (14.5) on the
//! demo design, showing the SafetyCase, static validation, approval revalidation
//! at promotion and the L0→L2 maturity progression.

use std::collections::BTreeMap;

use entelechy_assurance::{AssuranceCompiler, AssuranceReport, Claim, ClaimStatus, Evidence, SafetyCase};
use entelechy_eval::ConstraintClass;
use entelechy_identity::{Approval, ApprovalBinding, KeyRing, Principal, PrincipalKind, FixedClock};
use entelechy_release::{
    promote, ApprovalCheck, MaturityLevel, PromotionGate, ReleaseBundle,
};

/// Build the SafetyCase for the demo design: the refund negative goal is
/// structural (enforced by absent authority, proven by IR-I2), and a behavioral
/// disclosure goal passes an upper-bound test.
fn demo_safety_case() -> SafetyCase {
    SafetyCase {
        claims: vec![
            Claim {
                id: "C-no-refund".into(),
                statement: "the system never issues a refund".into(),
                class: ConstraintClass::Structural,
                evidence: vec![Evidence::StaticProof { invariant: "IR-I2".into() }],
            },
            Claim {
                id: "C-no-disclosure".into(),
                statement: "no cross-customer disclosure".into(),
                class: ConstraintClass::Behavioral,
                evidence: vec![Evidence::UpperBoundTest { upper_bound: 0.006, epsilon: 0.01, n: 300 }],
            },
        ],
        known_limitations: vec!["mock model provides no semantic quality signal".into()],
    }
}

fn demo_report() -> AssuranceReport {
    let program = crate::demo::demo_program();
    let catalog = crate::demo::demo_catalog();
    AssuranceCompiler::compile(&program, &catalog, demo_safety_case(), 1)
}

/// `entelechy assure`: run the assurance compiler and print the report.
pub fn cmd_assure() -> anyhow::Result<()> {
    let report = demo_report();
    println!("Assurance report (PRD 14.1):");
    println!("  Static IR violations: {}", report.static_violations.len());
    for v in &report.static_violations {
        println!("    {} at {}: {}", v.code, v.node_id, v.message);
    }
    println!("  SafetyCase claims:");
    for claim in &report.safety_case.claims {
        let status = match claim.status() {
            ClaimStatus::Proven => "proven (structural)".to_string(),
            ClaimStatus::StatisticallySupported => "statistically supported (behavioral)".into(),
            ClaimStatus::OperationallyBounded => "operationally bounded".into(),
            ClaimStatus::Insufficient { reason } => format!("INSUFFICIENT — {reason}"),
        };
        println!("    {} [{}] — {}", claim.id, claim.statement, status);
    }
    println!(
        "  Structural enforcement share: {:.0}%",
        report.safety_case.structural_enforcement_share() * 100.0
    );
    println!("  Release-eligible: {}", report.release_eligible());
    Ok(())
}

/// `entelechy release`: build a bundle and promote L0→L2 through the gate.
pub fn cmd_release() -> anyhow::Result<()> {
    let program = crate::demo::demo_program();
    let design_id = entelechy_artifacts::ArtifactId::of(&program)?.to_string();
    let authority_id = entelechy_artifacts::ArtifactId::of(&program.authority)?.to_string();

    let bundle = ReleaseBundle {
        design_id: design_id.clone(),
        goalspec_id: "sha256:goalspec-demo".into(),
        authority_id: authority_id.clone(),
        evalcontract_id: "sha256:evalcontract-demo".into(),
        safety_case: demo_safety_case(),
        policy_snapshot_version: 1,
        model_tool_ids: vec!["mock-small@mock-r1".into()],
        provenance: "study demo-run-1".into(),
        rollback_target: Some("rel-baseline".into()),
        no_rollback_rationale: None,
    };
    println!("Release bundle {} (design {design_id}).", bundle.id());

    // Assurance gate inputs.
    let report = demo_report();
    let gate = PromotionGate {
        assurance: &report,
        holdout_passed: true,
        negative_goal_clean: true,
    };

    // A hash-bound approval over the design + authority (PRD 14.5).
    let mut keyring = KeyRing::new();
    keyring.add_key("release-key-1", "operator-secret");
    let binding: ApprovalBinding = BTreeMap::from([
        ("design".to_string(), design_id),
        ("authority".to_string(), authority_id),
    ]);
    let now = 1_000_000u64;
    let approval = Approval::sign(
        Principal::new("approver@example", PrincipalKind::Approver),
        binding.clone(),
        "release-key-1",
        &keyring,
        now,
        Some(now + 30 * 24 * 60 * 60),
    )
    .expect("signing key present");
    let clock = FixedClock(Some(now + 60));
    let approvals = [approval];
    let check = ApprovalCheck {
        approvals: &approvals,
        current_binding: &binding,
        keyring: &keyring,
        clock: &clock,
    };

    // Promote L0 -> L1 -> L2 (PRD 14.2, 14.3).
    let mut level = MaturityLevel::L0;
    println!("Start at {level:?} — {}", level.required_evidence());
    for target in [MaturityLevel::L1, MaturityLevel::L2] {
        match promote(&bundle, level, target, &gate, &check, None) {
            Ok(p) => {
                level = p.to;
                println!("Promoted to {:?} — {}", p.to, p.to.required_evidence());
            }
            Err(e) => {
                println!("Promotion to {target:?} refused: {e}");
                break;
            }
        }
    }

    // Show the gate blocking a promotion when the holdout regresses.
    let regressed = PromotionGate {
        assurance: &report,
        holdout_passed: false,
        negative_goal_clean: true,
    };
    match promote(&bundle, MaturityLevel::L2, MaturityLevel::L3, &regressed, &check, None) {
        Ok(_) => println!("(unexpected) L3 promotion allowed despite holdout regression"),
        Err(e) => println!("Holdout regression correctly blocks L3: {e}"),
    }

    println!("\nApprovals are revalidated at promotion and go stale on any material change (PRD 14.5).");
    Ok(())
}
