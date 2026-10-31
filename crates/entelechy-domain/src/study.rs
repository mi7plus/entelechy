//! The Phase-0 real-domain study driver (PRD §21, 21.1).
//!
//! [`real_study`] runs the whole thing end to end on the resettable
//! [`crate::world::HelpdeskWorld`]:
//!
//! 1. a **comparator** (baseline vs a repaired design) scored by programmatic
//!    state checkers, not design structure;
//! 2. a **holdout gate** (EV-14) over the same real executor;
//! 3. **no silent pooling across material drift** (PV-2): evidence is kept in
//!    per-[`ModelFingerprint`] epochs and the release reads only the current one;
//! 4. a formal **single-deployment-profile declaration** with a signed approval
//!    (DP-1); and
//! 5. a content-addressed **reproduction packet** that re-runs to the same counts.
//!
//! It returns a [`RealStudyReport`] — one boolean per invariant plus the
//! sub-reports — exactly like the operational drills, so the CLI and the
//! phase-gate can both consume it.

use std::collections::BTreeMap;

use entelechy_artifacts::{ArtifactId, DeploymentProfile};
use entelechy_eval::{
    CallerIdentity, ConstraintClass, Difficulty, EvalContract, GateResponse, HoldoutVault,
    NegativeGoal, Plane, Provenance, ReleaseRule, RiskClass, Split, SplitPolicy, Suite, Task,
};
use entelechy_gateway::{MockModel, ModelGateway, ModelRequest, ProviderIdentity};
use entelechy_identity::{
    Approval, ApprovalBinding, ApprovalStatus, FixedClock, KeyRing, Principal, PrincipalKind,
    TimeSource,
};
use entelechy_ir::{AuthorityEnvelope, LlmNode, Node, NodeKind, Program, ToolNode, VerifyNode};
use serde::{Deserialize, Serialize};

use crate::epoch::{EvidenceEpoch, EvidenceLedger, ModelFingerprint, RevisionModel};
use crate::exec::{compare, execute, resolves, ComparisonReport};

/// The logical model this study drives.
const STUDY_MODEL: &str = "mock-small";

/// The common authority envelope: the agent may draft and resolve, and `refund`
/// is a structural negative goal (forbidden) it can never exercise.
fn authority() -> AuthorityEnvelope {
    let mut authority = AuthorityEnvelope::empty();
    authority.capabilities.insert("draft_reply".into());
    authority.capabilities.insert("resolve_part".into());
    authority.forbidden_capabilities.insert("refund".into());
    authority
}

/// The baseline single-agent design: it drafts a reply and resolves the first
/// part of a request — nothing grounds the reply and nothing resolves the rest, so
/// both failure classes remain.
pub fn baseline_design() -> Program {
    Program::new(
        authority(),
        Node::new(
            "root",
            NodeKind::Seq(vec![
                Node::new(
                    "draft",
                    NodeKind::Llm(LlmNode {
                        model: STUDY_MODEL.into(),
                        prompt_template: "resolve ticket: {input}".into(),
                        temperature: 0.0,
                    }),
                ),
                Node::new(
                    "resolve-first",
                    NodeKind::Tool(ToolNode {
                        capability: "resolve_part".into(),
                        args: serde_json::json!({ "part": 0 }),
                    }),
                ),
            ]),
        ),
    )
}

/// The repaired design: adds a verification step that grounds the reply (fixes the
/// reply class) and a parallel committee of workers that resolves every part
/// (fixes the decomposition class).
pub fn repaired_design() -> Program {
    Program::new(
        authority(),
        Node::new(
            "root",
            NodeKind::Seq(vec![
                Node::new(
                    "draft",
                    NodeKind::Llm(LlmNode {
                        model: STUDY_MODEL.into(),
                        prompt_template: "resolve ticket: {input}".into(),
                        temperature: 0.0,
                    }),
                ),
                Node::new(
                    "verify",
                    NodeKind::Verify(VerifyNode {
                        checker: "reply_grounded".into(),
                    }),
                ),
                Node::new(
                    "committee",
                    NodeKind::Par(vec![worker("w0", 0), worker("w1", 1), worker("w2", 2)]),
                ),
            ]),
        ),
    )
}

/// A design that tries to issue a refund — used to prove the structural negative
/// goal holds: the authority forbids the capability and the world refuses it, so
/// no refund is ever issued.
pub fn refund_attempting_design() -> Program {
    Program::new(
        authority(),
        Node::new(
            "root",
            NodeKind::Seq(vec![
                Node::new(
                    "draft",
                    NodeKind::Llm(LlmNode {
                        model: STUDY_MODEL.into(),
                        prompt_template: "resolve ticket: {input}".into(),
                        temperature: 0.0,
                    }),
                ),
                Node::new(
                    "bad-refund",
                    NodeKind::Tool(ToolNode {
                        capability: "refund".into(),
                        args: serde_json::json!({}),
                    }),
                ),
            ]),
        ),
    )
}

/// A committee worker that resolves one part.
fn worker(id: &str, part: u64) -> Node {
    Node::new(
        id,
        NodeKind::Tool(ToolNode {
            capability: "resolve_part".into(),
            args: serde_json::json!({ "part": part }),
        }),
    )
}

/// The real helpdesk suite (hand-authored seeds, PRD Q2), split 12/6/12. Tickets
/// are either `needs_reply` (fixed by grounding) or `needs_decomposition` with
/// three parts (fixed by the committee).
pub fn helpdesk_suite() -> Suite {
    let mut tasks = Vec::new();
    let mut push = |split: Split, n: usize, tag: &str, base: &str, parts: Option<u64>| {
        for i in 0..n {
            let mut input = serde_json::json!({ "ticket": format!("{base}{i}") });
            if let Some(p) = parts {
                input["parts"] = serde_json::json!(p);
            }
            tasks.push(Task {
                id: format!("{base}{i}"),
                input,
                environment: "helpdesk".into(),
                checkers: vec!["helpdesk.resolved".into()],
                tags: vec![tag.to_string()],
                difficulty: Difficulty::Medium,
                provenance: Provenance::HumanSeed,
                split,
                source_task: None,
            });
        }
    };
    push(Split::Tune, 7, "needs_reply", "tune-reply", None);
    push(Split::Tune, 5, "needs_decomposition", "tune-dec", Some(3));
    push(Split::Validation, 3, "needs_reply", "val-reply", None);
    push(
        Split::Validation,
        3,
        "needs_decomposition",
        "val-dec",
        Some(3),
    );
    push(Split::Holdout, 7, "needs_reply", "hold-reply", None);
    push(
        Split::Holdout,
        5,
        "needs_decomposition",
        "hold-dec",
        Some(3),
    );
    Suite::new(tasks)
}

/// The eval contract for the study (PRD 9, 21.1).
fn contract() -> EvalContract {
    EvalContract {
        version: 1,
        criteria: vec!["resolves_tier1_ticket".into()],
        negative_goals: vec![NegativeGoal {
            name: "no_refund".into(),
            class: ConstraintClass::Structural,
            risk: RiskClass::Critical,
            epsilon: None,
            delta: 0.05,
        }],
        splits: SplitPolicy {
            tune: 12,
            validation: 6,
            holdout: 12,
        },
        release: ReleaseRule {
            primary_metric: "task_success".into(),
            target_improvement_pp: 20.0,
            holdout_query_budget: 5,
        },
    }
}

/// A formal declaration that an installation runs under exactly one deployment
/// profile, with a signed approval of that choice (PRD 17.1, DP-1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentDeclaration {
    /// The single declared profile; guarantees apply only within it.
    pub profile: DeploymentProfile,
    /// The profile's one-line guarantee boundary.
    pub guarantee_boundary: String,
    /// The signed approval binding this exact profile (the DP-1 approval step).
    pub approval: Approval,
}

impl DeploymentDeclaration {
    /// Declare and approve a single deployment profile (DP-1). Signs an approval
    /// bound to the chosen profile with `key_id` from `keyring`; a failed signature
    /// yields an unverifiable approval (so the declaration reads as not approved).
    pub fn declare(
        profile: DeploymentProfile,
        approver: &str,
        keyring: &KeyRing,
        key_id: &str,
        approved_at: u64,
        expires_at: Option<u64>,
    ) -> Self {
        let principal = Principal::new(approver, PrincipalKind::DomainOwner);
        let binding = Self::binding(profile);
        let approval = Approval::sign(
            principal.clone(),
            binding.clone(),
            key_id,
            keyring,
            approved_at,
            expires_at,
        )
        .unwrap_or_else(|| Approval {
            approver: principal,
            binding,
            key_id: key_id.to_string(),
            signature: String::new(),
            approved_at,
            expires_at,
        });
        Self {
            profile,
            guarantee_boundary: profile.guarantee_boundary().to_string(),
            approval,
        }
    }

    /// The approval binding for a profile: exactly one declared profile.
    fn binding(profile: DeploymentProfile) -> ApprovalBinding {
        BTreeMap::from([(
            "deployment_profile".to_string(),
            profile_label(profile).to_string(),
        )])
    }

    /// Whether the declaration's approval is currently valid for its profile
    /// (revalidated like any approval — signature, revocation, expiry, binding).
    pub fn is_approved(&self, keyring: &KeyRing, clock: &dyn TimeSource) -> bool {
        self.approval
            .status_for(&Self::binding(self.profile), keyring, clock)
            == ApprovalStatus::Valid
    }
}

/// Stable label for a deployment profile (used in the approval binding).
fn profile_label(profile: DeploymentProfile) -> &'static str {
    match profile {
        DeploymentProfile::Local => "local",
        DeploymentProfile::SingleNodeServer => "single-node-server",
        DeploymentProfile::Cluster => "cluster",
    }
}

/// A content-addressed packet that pins everything needed to reproduce the study's
/// resolved counts (PRD principle 6; 21.1 reproducibility).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReproductionPacket {
    /// Content address of the suite.
    pub suite_id: String,
    /// Content address of the baseline design.
    pub baseline_id: String,
    /// Content address of the candidate design.
    pub candidate_id: String,
    /// The model fingerprint the counts were produced under.
    pub fingerprint: ModelFingerprint,
    /// The programmatic checker that decided resolution.
    pub checker: String,
    /// Sealed baseline resolved count over the full suite.
    pub baseline_resolved: usize,
    /// Sealed candidate resolved count over the full suite.
    pub candidate_resolved: usize,
    /// Total tasks in the suite.
    pub total: usize,
    /// Content address of the packet contents above.
    pub packet_id: String,
}

impl ReproductionPacket {
    /// Seal a reproduction packet from a comparison over the whole suite.
    pub fn seal(
        suite: &Suite,
        baseline: &Program,
        candidate: &Program,
        fingerprint: &ModelFingerprint,
        comparison: &ComparisonReport,
    ) -> Self {
        let suite_id = content_id(suite);
        let baseline_id = content_id(baseline);
        let candidate_id = content_id(candidate);
        let checker = "helpdesk.resolved".to_string();
        let contents = serde_json::json!({
            "suite_id": suite_id,
            "baseline_id": baseline_id,
            "candidate_id": candidate_id,
            "fingerprint": fingerprint.as_str(),
            "checker": checker,
            "baseline_resolved": comparison.baseline_resolved,
            "candidate_resolved": comparison.candidate_resolved,
            "total": comparison.total,
        });
        let packet_id = content_id(&contents);
        Self {
            suite_id,
            baseline_id,
            candidate_id,
            fingerprint: fingerprint.clone(),
            checker,
            baseline_resolved: comparison.baseline_resolved,
            candidate_resolved: comparison.candidate_resolved,
            total: comparison.total,
            packet_id,
        }
    }

    /// Re-run the study's comparison and confirm it reproduces the sealed packet:
    /// the suite and designs hash identically and the resolved counts match.
    pub fn reproduce(
        &self,
        suite: &Suite,
        baseline: &Program,
        candidate: &Program,
        gateway: &dyn ModelGateway,
    ) -> Result<(), ReproductionError> {
        check_id("suite", &self.suite_id, &content_id(suite))?;
        check_id("baseline", &self.baseline_id, &content_id(baseline))?;
        check_id("candidate", &self.candidate_id, &content_id(candidate))?;
        let tasks: Vec<&Task> = suite.tasks.iter().collect();
        let cmp = compare(baseline, candidate, &tasks, gateway);
        if cmp.baseline_resolved != self.baseline_resolved
            || cmp.candidate_resolved != self.candidate_resolved
        {
            return Err(ReproductionError::CountMismatch {
                sealed_baseline: self.baseline_resolved,
                sealed_candidate: self.candidate_resolved,
                got_baseline: cmp.baseline_resolved,
                got_candidate: cmp.candidate_resolved,
            });
        }
        Ok(())
    }
}

/// Why a reproduction attempt did not match the sealed packet.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReproductionError {
    /// A content address no longer matches (the suite or a design changed).
    #[error("{what} id changed: sealed {sealed}, got {got}")]
    IdMismatch {
        /// Which artifact changed.
        what: String,
        /// The sealed id.
        sealed: String,
        /// The recomputed id.
        got: String,
    },
    /// The resolved counts differ (the domain is no longer deterministic).
    #[error(
        "resolved counts changed: sealed b={sealed_baseline} c={sealed_candidate}, \
         got b={got_baseline} c={got_candidate}"
    )]
    CountMismatch {
        /// Sealed baseline count.
        sealed_baseline: usize,
        /// Sealed candidate count.
        sealed_candidate: usize,
        /// Recomputed baseline count.
        got_baseline: usize,
        /// Recomputed candidate count.
        got_candidate: usize,
    },
}

fn check_id(what: &str, sealed: &str, got: &str) -> Result<(), ReproductionError> {
    if sealed == got {
        Ok(())
    } else {
        Err(ReproductionError::IdMismatch {
            what: what.to_string(),
            sealed: sealed.to_string(),
            got: got.to_string(),
        })
    }
}

/// Content address of any serializable value (`sha256:…`).
fn content_id<T: Serialize>(value: &T) -> String {
    ArtifactId::of(value)
        .map(|i| i.to_string())
        .unwrap_or_default()
}

/// The full real-study report: one boolean per invariant plus the sub-reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealStudyReport {
    /// Baseline vs repaired comparison on the real domain (tune + validation).
    pub comparison: ComparisonReport,
    /// Whether the repaired design strictly improves with no regression.
    pub candidate_improves: bool,
    /// Whether the refund negative goal held across every task.
    pub negative_goal_held: bool,
    /// The holdout gate verdict (`pass`/`fail`/`refused`).
    pub holdout_result: String,
    /// Whether the holdout gate passed (EV-14).
    pub holdout_passed: bool,
    /// Whether a material model drift was observed (PV-2).
    pub drift_detected: bool,
    /// The evidence epochs, one per fingerprint.
    pub epochs: Vec<EvidenceEpoch>,
    /// Resolved count the release relies on (current epoch only).
    pub release_successes: usize,
    /// Total observations the release relies on (current epoch only).
    pub release_total: usize,
    /// Naive pooled resolved count across all epochs (the number PV-2 forbids).
    pub pooled_successes: usize,
    /// Naive pooled total across all epochs.
    pub pooled_total: usize,
    /// Whether evidence was kept epoch-scoped (no silent pooling across drift).
    pub no_silent_pooling: bool,
    /// Whether pooling across the drift would have masked a regression.
    pub pooling_would_mask_regression: bool,
    /// The single-deployment-profile declaration (DP-1).
    pub deployment: DeploymentDeclaration,
    /// Whether the single-profile approval is valid (DP-1).
    pub single_profile_approved: bool,
    /// The content-addressed reproduction packet.
    pub reproduction: ReproductionPacket,
    /// Whether the study reproduced to the sealed counts.
    pub reproduced: bool,
    /// Overall: every invariant held.
    pub ok: bool,
}

/// Run the Phase-0 real-domain study end to end (PRD §21, 21.1).
pub fn real_study() -> RealStudyReport {
    let suite = helpdesk_suite();
    let baseline = baseline_design();
    let repaired = repaired_design();
    // The calibrated gateway: the mock's revision (`mock-r1`) is what the
    // verification step is grounded against.
    let calibrated = MockModel::new();

    // 1. Real-domain comparator over the non-holdout tasks.
    let working: Vec<&Task> = suite
        .tasks
        .iter()
        .filter(|t| matches!(t.split, Split::Tune | Split::Validation))
        .collect();
    let comparison = compare(&baseline, &repaired, &working, &calibrated);
    let candidate_improves = comparison.candidate_dominates();

    // Negative goal: even a refund-attempting design never issues a refund.
    let probe = refund_attempting_design();
    let negative_goal_held = suite
        .tasks
        .iter()
        .all(|t| execute(&probe, t, &calibrated).world.negative_goal_held());

    // 2. PV-2: evidence epochs across a material drift. Run the repaired design
    // over the validation split under the calibrated revision, then under a
    // silently-drifted one.
    let val: Vec<&Task> = suite.split(Split::Validation).collect();
    let calibrated_rev = RevisionModel::new("mock-r1");
    let drifted_rev = RevisionModel::new("mock-r2");
    let fp_calibrated = gateway_fingerprint(&calibrated_rev);
    let fp_drifted = gateway_fingerprint(&drifted_rev);
    let out_calibrated = epoch_outcomes(&repaired, &val, &calibrated_rev);
    let out_drifted = epoch_outcomes(&repaired, &val, &drifted_rev);

    let mut ledger = EvidenceLedger::new();
    ledger.observe(&fp_calibrated, &out_calibrated);
    ledger.observe(&fp_drifted, &out_drifted);
    let drift_detected = ledger.drift_detected();
    let pooled_successes = ledger.pooled_successes();
    let pooled_total = ledger.pooled_total();
    let release = ledger.release_evidence().cloned().unwrap_or(EvidenceEpoch {
        fingerprint: fp_drifted.clone(),
        outcomes: Vec::new(),
    });
    let release_successes = release.successes();
    let release_total = release.total();
    // No silent pooling: a drift opened a second epoch, the release reads only the
    // post-drift epoch, and that epoch is strictly smaller than the pooled history.
    let no_silent_pooling =
        drift_detected && release.fingerprint == fp_drifted && pooled_total > release_total;
    let pooling_would_mask_regression = ledger.pooling_would_mask_regression();

    // 3. Holdout gate (EV-14) over the same real executor.
    let (holdout_result, holdout_passed) = run_holdout_gate(&baseline, &repaired, &suite);

    // 4. DP-1: declare exactly one deployment profile with a signed approval.
    let mut keyring = KeyRing::new();
    keyring.add_key("dp-key", "deploy-secret");
    let clock = FixedClock(Some(1_700_000_000));
    let deployment = DeploymentDeclaration::declare(
        DeploymentProfile::Local,
        "domain-owner",
        &keyring,
        "dp-key",
        1_700_000_000,
        Some(2_000_000_000),
    );
    let single_profile_approved = deployment.is_approved(&keyring, &clock);

    // 5. Reproduction packet over the whole suite (under the calibrated model).
    let all_tasks: Vec<&Task> = suite.tasks.iter().collect();
    let full = compare(&baseline, &repaired, &all_tasks, &calibrated);
    let reproduction =
        ReproductionPacket::seal(&suite, &baseline, &repaired, &fp_calibrated, &full);
    let reproduced = reproduction
        .reproduce(&suite, &baseline, &repaired, &calibrated)
        .is_ok();

    let ok = candidate_improves
        && negative_goal_held
        && holdout_passed
        && drift_detected
        && no_silent_pooling
        && single_profile_approved
        && reproduced;

    RealStudyReport {
        comparison,
        candidate_improves,
        negative_goal_held,
        holdout_result,
        holdout_passed,
        drift_detected,
        epochs: ledger.epochs().to_vec(),
        release_successes,
        release_total,
        pooled_successes,
        pooled_total,
        no_silent_pooling,
        pooling_would_mask_regression,
        deployment,
        single_profile_approved,
        reproduction,
        reproduced,
        ok,
    }
}

/// Resolve every task under a gateway and collect the outcomes.
fn epoch_outcomes(design: &Program, tasks: &[&Task], gateway: &dyn ModelGateway) -> Vec<bool> {
    tasks.iter().map(|t| resolves(design, t, gateway)).collect()
}

/// The fingerprint a gateway produces (from a real identity-probe call).
fn gateway_fingerprint(gateway: &dyn ModelGateway) -> ModelFingerprint {
    let req = ModelRequest {
        model: STUDY_MODEL.into(),
        prompt: "identity-probe".into(),
        temperature: 0.0,
    };
    match gateway.infer(&req) {
        Ok(resp) => ModelFingerprint::of(&resp.identity),
        Err(_) => ModelFingerprint::of(&ProviderIdentity {
            provider: "unknown".into(),
            advertised_model: String::new(),
            endpoint: String::new(),
            request_schema_version: String::new(),
            sampling: serde_json::Value::Null,
            revision: None,
        }),
    }
}

/// Gate the repaired candidate against the baseline on the sealed holdout using
/// the real executor as the scoring function (EV-14).
fn run_holdout_gate(baseline: &Program, repaired: &Program, suite: &Suite) -> (String, bool) {
    let contract = contract();
    let holdout: Vec<Task> = suite.split(Split::Holdout).cloned().collect();
    let mut vault = match HoldoutVault::seal(&contract, holdout) {
        Ok(v) => v,
        Err(_) => return ("refused".to_string(), false),
    };
    let assurance = CallerIdentity {
        id: "assure".into(),
        plane: Plane::Assurance,
    };
    let candidate_hash = content_id(repaired);
    let base = baseline.clone();
    let cand = repaired.clone();
    let gw_base = MockModel::new();
    let gw_cand = MockModel::new();
    let resp = vault.gate_query(
        &assurance,
        &candidate_hash,
        &contract,
        &move |t| resolves(&base, t, &gw_base),
        &move |t| resolves(&cand, t, &gw_cand),
    );
    match resp {
        GateResponse::Pass { .. } => ("pass".to_string(), true),
        GateResponse::Fail { .. } => ("fail".to_string(), false),
        GateResponse::Refused { .. } => ("refused".to_string(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_study_is_green() {
        let r = real_study();
        assert!(
            r.candidate_improves,
            "repaired design improves on the baseline"
        );
        assert!(r.negative_goal_held, "refund is never issued");
        assert!(
            r.holdout_passed,
            "the holdout gate passes: {}",
            r.holdout_result
        );
        assert!(r.drift_detected, "a material drift was observed");
        assert!(r.no_silent_pooling, "evidence stayed epoch-scoped");
        assert!(
            r.pooling_would_mask_regression,
            "pooling across the drift would have masked the regression"
        );
        assert!(
            r.single_profile_approved,
            "the deployment profile is approved"
        );
        assert!(r.reproduced, "the study reproduced to the sealed counts");
        assert!(r.ok, "every invariant held");
    }

    #[test]
    fn real_study_is_deterministic() {
        assert_eq!(real_study(), real_study());
    }

    #[test]
    fn release_evidence_is_the_post_drift_epoch_not_the_pool() {
        let r = real_study();
        // Validation split is 3 reply + 3 decomposition. Calibrated: 6/6.
        // Drifted: the 3 reply tickets lose grounding -> 3/6.
        assert_eq!(r.epochs.len(), 2);
        assert_eq!(r.release_successes, 3);
        assert_eq!(r.release_total, 6);
        assert_eq!(r.pooled_successes, 9);
        assert_eq!(r.pooled_total, 12);
    }

    #[test]
    fn a_tampered_reproduction_packet_is_caught() {
        let suite = helpdesk_suite();
        let baseline = baseline_design();
        let repaired = repaired_design();
        let gw = MockModel::new();
        let all: Vec<&Task> = suite.tasks.iter().collect();
        let cmp = compare(&baseline, &repaired, &all, &gw);
        let fp = gateway_fingerprint(&gw);
        let mut packet = ReproductionPacket::seal(&suite, &baseline, &repaired, &fp, &cmp);
        // Forge the sealed candidate count: reproduction must reject it.
        packet.candidate_resolved += 1;
        let err = packet
            .reproduce(&suite, &baseline, &repaired, &gw)
            .unwrap_err();
        assert!(matches!(err, ReproductionError::CountMismatch { .. }));
    }

    #[test]
    fn declaration_is_stale_if_the_profile_changes() {
        let mut keyring = KeyRing::new();
        keyring.add_key("k", "s");
        let clock = FixedClock(Some(100));
        let decl = DeploymentDeclaration::declare(
            DeploymentProfile::Local,
            "owner",
            &keyring,
            "k",
            10,
            Some(10_000),
        );
        assert!(decl.is_approved(&keyring, &clock));
        // An approval bound to `Local` is not valid for a `Cluster` declaration.
        let mut forged = decl.clone();
        forged.profile = DeploymentProfile::Cluster;
        assert!(
            !forged.is_approved(&keyring, &clock),
            "the approval binds exactly one profile"
        );
    }
}
