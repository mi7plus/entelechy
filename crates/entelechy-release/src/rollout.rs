//! Staged rollout: the shadow (L3) and canary (L4) stages of the promotion path
//! (PRD §14.2 maturity levels, §21.4 Phase 4).
//!
//! [`promote`] gates the maturity *levels* abstractly; this module
//! is the operational machinery of the two staged-rollout levels between a
//! safety-qualified candidate (L2) and production (L5):
//!
//! - **Shadow (L3)** — "production inputs observed without consequential effects"
//!   (PRD §14.2). L3 requires *effect substitution*: reads may hit live systems,
//!   while writes and external actions go to a dry-run / record-only sink, so the
//!   candidate commits **no** consequential effects. A capability that declares no
//!   dry-run support cannot run in shadow, so [`shadow_eligibility`] fails closed on
//!   any consequential effect lacking `dry_run_supported`; such a design can only
//!   enter canary through a recorded override (§14.2). [`run_shadow`] replays real
//!   production inputs through both the active baseline and the candidate, compares
//!   their outcomes, and counts every candidate consequential effect as
//!   *substituted* — there is no path in the report for a candidate effect to
//!   commit, which is the structural statement of "without consequential effects".
//!
//! - **Canary (L4)** — "bounded production authority and monitored rollout"
//!   (PRD §14.2). A bounded fraction of real traffic is routed to the candidate
//!   (minimal authority), using the same deterministic
//!   [`sample_for_eval`] the online loop uses, so the
//!   routing is reproducible per run id. The candidate slice is monitored against an
//!   SLO [`error budget`](crate::ErrorBudgetAccount), drift of the monitored metric
//!   against the baseline slice ([`detect_drift`]), and
//!   the candidate's [`Correctness`] verdict. [`run_canary`]
//!   turns those into a [`CanaryVerdict`]: promote, hold, or roll back.
//!
//! [`drive_staged_promotion`] feeds a canary verdict into the **real** release gate:
//! a healthy canary is only a *precondition* for promotion — the
//! [`promote`] gate still revalidates approvals, assurance, the
//! holdout and rollback readiness, so a healthy canary never bypasses the gate (the
//! same principle as "a re-study never auto-promotes"). A regressing canary fires a
//! rollback through the [`DeploymentController`] under
//! separated stop authority (PRD §14.6).
//!
//! [`rollout_drill`] composes all of this end to end into one deterministic report,
//! mirroring [`slo_drill`](crate::slo_drill) and [`rollback_drill`](crate::rollback_drill).

use serde::{Deserialize, Serialize};

use entelechy_ir::{EffectClass, EffectMetadata};
use entelechy_ops::{detect_drift, sample_for_eval, DriftEvidence, DriftKind};

use crate::{
    promote, ApprovalCheck, AutoTrigger, Correctness, DeployAuthority, DeployError,
    DeploymentController, ErrorBudgetAccount, MaturityLevel, Promotion, PromotionError,
    PromotionGate, ReleaseBundle, ServiceLevelObjective,
};

/// Whether a design may run in shadow (PRD §14.2). Shadow requires effect
/// substitution, so every *consequential* effect the design uses must support a
/// dry-run; one that does not makes the design shadow-ineligible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "eligibility")]
pub enum ShadowEligibility {
    /// Every consequential effect supports dry-run substitution — shadow may run.
    Eligible,
    /// One or more consequential effects cannot be substituted (no dry-run). Shadow
    /// is refused; entering canary (L4) requires a recorded override (§14.2). The
    /// listed operation-key namespaces identify the blocking effects.
    Ineligible {
        /// The `operation_key_namespace` of each consequential effect lacking a
        /// dry-run, in input order.
        blocking: Vec<String>,
    },
}

impl ShadowEligibility {
    /// Whether the design may run in shadow.
    pub fn is_eligible(&self) -> bool {
        matches!(self, ShadowEligibility::Eligible)
    }
}

/// Decide whether a design may run in shadow (PRD §14.2). A design is shadow-eligible
/// iff every *consequential* (write / external / irreversible) effect it uses
/// declares `dry_run_supported`; a read-only effect needs no substitution. Fails
/// closed: any consequential effect without a dry-run makes the design ineligible.
pub fn shadow_eligibility(effects: &[EffectMetadata]) -> ShadowEligibility {
    let blocking: Vec<String> = effects
        .iter()
        .filter(|e| e.class.is_consequential() && !e.dry_run_supported)
        .map(|e| e.operation_key_namespace.clone())
        .collect();
    if blocking.is_empty() {
        ShadowEligibility::Eligible
    } else {
        ShadowEligibility::Ineligible { blocking }
    }
}

/// What one release produced for one production input, abstracted so the rollout
/// machinery stays domain-agnostic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunOutcome {
    /// A comparable result token: equal tokens mean the two releases agreed.
    pub result: String,
    /// Whether the run errored.
    pub error: bool,
    /// The monitored signal for this run (latency, cost, a quality score) — the
    /// value the canary's SLO/drift analysis consumes.
    pub metric: f64,
    /// How many *consequential* effects this run would commit in production. In
    /// shadow these are substituted (never committed); in canary they commit for the
    /// routed slice.
    pub consequential_effects: u32,
}

impl RunOutcome {
    /// A succeeding run with one result token, metric and no consequential effects.
    pub fn ok(result: impl Into<String>, metric: f64) -> Self {
        Self {
            result: result.into(),
            error: false,
            metric,
            consequential_effects: 0,
        }
    }

    /// Set the number of consequential effects this run would commit.
    pub fn with_effects(mut self, n: u32) -> Self {
        self.consequential_effects = n;
        self
    }

    /// Mark this run as errored.
    pub fn failed(mut self) -> Self {
        self.error = true;
        self
    }
}

/// One production input and the outcome each release produced for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RolloutObservation {
    /// The input's run id (drives deterministic canary routing).
    pub run_id: String,
    /// What the active baseline release produced (the one serving production).
    pub baseline: RunOutcome,
    /// What the candidate produced (shadowed, then canaried).
    pub candidate: RunOutcome,
}

/// Policy for the shadow stage (PRD §14.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowConfig {
    /// Minimum shadow samples before a decision (too few is inconclusive).
    pub min_samples: usize,
    /// Minimum candidate/baseline agreement rate to proceed to canary.
    pub min_agreement: f64,
}

impl Default for ShadowConfig {
    fn default() -> Self {
        Self {
            min_samples: 20,
            min_agreement: 0.95,
        }
    }
}

/// Whether shadowing supports proceeding to canary (PRD §14.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "decision")]
pub enum ShadowDecision {
    /// Parity is good enough and the candidate introduced no new errors — ready to
    /// canary (L3 → L4).
    Proceed,
    /// Keep shadowing or investigate: too few samples, new candidate errors, or
    /// agreement below the floor. Carries the reason.
    Hold {
        /// Why shadowing did not clear the candidate for canary.
        reason: String,
    },
}

/// The outcome of the shadow stage (PRD §14.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShadowReport {
    /// Shadow inputs observed.
    pub samples: usize,
    /// Inputs where the candidate's result matched the baseline's.
    pub agreements: usize,
    /// Inputs where the candidate's result differed from the baseline's.
    pub divergences: usize,
    /// Inputs where the candidate errored but the baseline did not (new failures).
    pub candidate_new_errors: usize,
    /// Consequential effects the candidate *would* have committed, all substituted
    /// (dry-run / record-only) — the defining L3 property is that none reached the
    /// world.
    pub candidate_effects_substituted: u64,
    /// Consequential effects the baseline actually committed — production is still
    /// served by the active release during shadow.
    pub baseline_effects_committed: u64,
    /// Whether shadowing clears the candidate for canary.
    pub decision: ShadowDecision,
}

impl ShadowReport {
    /// The candidate/baseline agreement rate in `[0, 1]` (`1.0` with no samples).
    pub fn agreement_rate(&self) -> f64 {
        if self.samples == 0 {
            1.0
        } else {
            self.agreements as f64 / self.samples as f64
        }
    }
}

/// Run the shadow stage (PRD §14.2): replay production inputs through the baseline
/// and the candidate with the candidate's consequential effects *substituted*, and
/// decide whether the candidate is ready to canary. The candidate commits nothing —
/// its effects are only counted as substituted.
pub fn run_shadow(config: &ShadowConfig, observations: &[RolloutObservation]) -> ShadowReport {
    let mut agreements = 0;
    let mut divergences = 0;
    let mut candidate_new_errors = 0;
    let mut candidate_effects_substituted: u64 = 0;
    let mut baseline_effects_committed: u64 = 0;

    for obs in observations {
        if obs.candidate.result == obs.baseline.result {
            agreements += 1;
        } else {
            divergences += 1;
        }
        if obs.candidate.error && !obs.baseline.error {
            candidate_new_errors += 1;
        }
        // Effect substitution (§14.2): the candidate's consequential effects are
        // recorded as substituted, never committed; only the active baseline serves
        // production during shadow.
        candidate_effects_substituted += u64::from(obs.candidate.consequential_effects);
        baseline_effects_committed += u64::from(obs.baseline.consequential_effects);
    }

    let samples = observations.len();
    let agreement_rate = if samples == 0 {
        1.0
    } else {
        agreements as f64 / samples as f64
    };

    let decision = if samples < config.min_samples {
        ShadowDecision::Hold {
            reason: format!(
                "insufficient shadow samples: {samples} < {}",
                config.min_samples
            ),
        }
    } else if candidate_new_errors > 0 {
        ShadowDecision::Hold {
            reason: format!(
                "candidate errored on {candidate_new_errors} input(s) the baseline handled"
            ),
        }
    } else if agreement_rate < config.min_agreement {
        ShadowDecision::Hold {
            reason: format!(
                "agreement {:.3} below floor {:.3}",
                agreement_rate, config.min_agreement
            ),
        }
    } else {
        ShadowDecision::Proceed
    };

    ShadowReport {
        samples,
        agreements,
        divergences,
        candidate_new_errors,
        candidate_effects_substituted,
        baseline_effects_committed,
        decision,
    }
}

/// Policy for the canary stage (PRD §14.2).
#[derive(Clone, Debug, PartialEq)]
pub struct CanaryConfig {
    /// Bounded fraction of production traffic routed to the candidate (minimal
    /// authority). Routing is deterministic per run id via
    /// [`sample_for_eval`].
    pub traffic_fraction: f64,
    /// The SLO the candidate slice is measured against (its error budget).
    pub slo: ServiceLevelObjective,
    /// The monitored signal kind, recorded in drift evidence.
    pub drift_kind: DriftKind,
    /// Drift sensitivity in baseline standard deviations.
    pub drift_k: f64,
    /// Minimum runs routed to the candidate before a verdict (else hold — not enough
    /// signal to judge the rollout).
    pub min_canary_runs: usize,
}

/// What the canary analysis recommends (PRD §14.2, §14.6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "verdict")]
pub enum CanaryVerdict {
    /// The monitored slice is healthy — eligible to feed the L4 → L5 release gate
    /// (still subject to the full gate; never an auto-promotion).
    Promote,
    /// Inconclusive — too few canary runs to judge; keep canarying. Carries why.
    Hold {
        /// Why no promote/rollback decision could be made yet.
        reason: String,
    },
    /// A regression — fire a rollback under the named declared trigger (PRD §14.6).
    Rollback {
        /// The declared automatic-rollback trigger this regression maps to.
        trigger: AutoTrigger,
        /// A human-readable reason naming the breached signal.
        reason: String,
    },
}

/// The outcome of the canary stage (PRD §14.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanaryReport {
    /// Total production inputs seen.
    pub total_runs: usize,
    /// Inputs routed to the candidate (the bounded canary slice).
    pub canary_runs: usize,
    /// Inputs served by the baseline.
    pub baseline_runs: usize,
    /// The realized fraction routed to the candidate (`canary_runs / total_runs`).
    pub realized_fraction: f64,
    /// The candidate slice's SLO error-budget account.
    pub budget: ErrorBudgetAccount,
    /// Drift of the candidate slice's metric against the baseline slice, if any
    /// exceeded the sensitivity.
    pub drift: Option<DriftEvidence>,
    /// The analysis verdict.
    pub verdict: CanaryVerdict,
}

/// Run the canary stage (PRD §14.2): route a bounded fraction of inputs to the
/// candidate (which commits its consequential effects — bounded authority), measure
/// the slice against its SLO error budget and against drift of the monitored metric,
/// and combine with the candidate's `correctness` verdict into a [`CanaryVerdict`].
///
/// Correctness is evaluated *first* (a failed correctness gate rolls back regardless
/// of the operational signals, mirroring [`release_health`](crate::release_health)),
/// then — once enough canary runs exist — an exhausted error budget or a metric drift
/// rolls back; otherwise the canary promotes.
pub fn run_canary(
    config: &CanaryConfig,
    observations: &[RolloutObservation],
    correctness: Correctness,
) -> CanaryReport {
    let mut budget = ErrorBudgetAccount::for_slo(&config.slo);
    let mut baseline_window: Vec<f64> = Vec::new();
    let mut canary_window: Vec<f64> = Vec::new();
    let mut canary_runs = 0;

    for obs in observations {
        if sample_for_eval(&obs.run_id, config.traffic_fraction) {
            // Routed to the candidate: it commits its effects (bounded authority),
            // and its outcome is one SLO event in the canary slice.
            canary_runs += 1;
            budget.observe(1, u64::from(obs.candidate.error));
            canary_window.push(obs.candidate.metric);
        } else {
            // Served by the baseline — the drift comparison reference.
            baseline_window.push(obs.baseline.metric);
        }
    }

    let total_runs = observations.len();
    let baseline_runs = total_runs - canary_runs;
    let realized_fraction = if total_runs == 0 {
        0.0
    } else {
        canary_runs as f64 / total_runs as f64
    };
    let drift = detect_drift(
        config.drift_kind,
        &baseline_window,
        &canary_window,
        config.drift_k,
    );

    let verdict = if correctness == Correctness::Failed {
        CanaryVerdict::Rollback {
            trigger: AutoTrigger::NegativeGoalViolation,
            reason: "canary correctness gate failed".into(),
        }
    } else if canary_runs < config.min_canary_runs {
        CanaryVerdict::Hold {
            reason: format!(
                "insufficient canary runs: {canary_runs} < {}",
                config.min_canary_runs
            ),
        }
    } else if budget.is_exhausted() {
        CanaryVerdict::Rollback {
            trigger: AutoTrigger::SafetyMetricBreach,
            reason: format!(
                "canary error budget exhausted ({} bad / {} events)",
                budget.bad_events, budget.total_events
            ),
        }
    } else if let Some(ev) = &drift {
        CanaryVerdict::Rollback {
            trigger: AutoTrigger::SafetyMetricBreach,
            reason: format!("canary metric drift z={:.2}", ev.z_shift),
        }
    } else {
        CanaryVerdict::Promote
    };

    CanaryReport {
        total_runs,
        canary_runs,
        baseline_runs,
        realized_fraction,
        budget,
        drift,
        verdict,
    }
}

/// The result of feeding a canary verdict into the real release gate and deployment
/// controller (PRD §14.2, §14.3, §14.6).
#[derive(Debug, PartialEq)]
pub enum StagedOutcome {
    /// The canary was healthy and the real L4 → L5 gate promoted.
    Promoted(Promotion),
    /// The canary was healthy but the real gate refused (approvals, assurance,
    /// holdout or rollback readiness). A healthy canary is only a precondition — it
    /// never bypasses the gate.
    GateRefused(PromotionError),
    /// The canary was inconclusive — neither promoted nor rolled back.
    Held {
        /// Why the canary could not decide.
        reason: String,
    },
    /// The canary regressed and a rollback recovered the prior known-good release.
    RolledBack {
        /// The release the rollback recovered to.
        to: String,
        /// The declared trigger the regression mapped to.
        trigger: AutoTrigger,
    },
    /// The canary regressed but the rollback could not run (no target, or the actor
    /// lacked stop authority).
    RollbackUnavailable(DeployError),
}

/// Drive the staged-promotion decision from a canary report (PRD §14.2, §14.6):
///
/// - **Promote** → run the real [`promote`] gate for L4 → L5. The
///   gate still revalidates approvals, assurance, the holdout and rollback readiness,
///   so a healthy canary never bypasses it.
/// - **Hold** → do nothing (keep canarying).
/// - **Rollback** → roll the deployment back to the prior known-good release through
///   the [`DeploymentController`] under the given stop
///   authority (which must be separate from promote authority — PRD §14.6).
pub fn drive_staged_promotion(
    report: &CanaryReport,
    bundle: &ReleaseBundle,
    gate: &PromotionGate,
    approvals: &ApprovalCheck,
    deployment: &mut DeploymentController,
    stopper: DeployAuthority,
) -> StagedOutcome {
    match &report.verdict {
        CanaryVerdict::Promote => {
            match promote(
                bundle,
                MaturityLevel::L4,
                MaturityLevel::L5,
                gate,
                approvals,
                None,
            ) {
                Ok(p) => StagedOutcome::Promoted(p),
                Err(e) => StagedOutcome::GateRefused(e),
            }
        }
        CanaryVerdict::Hold { reason } => StagedOutcome::Held {
            reason: reason.clone(),
        },
        CanaryVerdict::Rollback { trigger, .. } => match deployment.rollback(stopper) {
            Ok(to) => StagedOutcome::RolledBack {
                to,
                trigger: *trigger,
            },
            Err(e) => StagedOutcome::RollbackUnavailable(e),
        },
    }
}

/// The outcome of the staged-rollout drill (PRD §14.2, §21.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolloutDrillReport {
    /// §14.2: a design whose consequential effects all support dry-run is
    /// shadow-eligible, and one with a consequential effect lacking dry-run is not
    /// (effect substitution is required to run in shadow).
    pub shadow_requires_dry_run: bool,
    /// §14.2: shadowing the candidate commits *none* of its consequential effects —
    /// they are all substituted — while the active baseline keeps serving production.
    pub shadow_commits_no_candidate_effects: bool,
    /// §14.2: a high-agreement, no-new-error shadow run clears the candidate for
    /// canary (proceed).
    pub shadow_parity_proceeds: bool,
    /// §14.2: a shadow run where the candidate introduces new errors holds (does not
    /// proceed to canary).
    pub shadow_divergence_holds: bool,
    /// §14.2: the canary routes only a *bounded* fraction of traffic to the
    /// candidate — some, but far from all — deterministically per run id.
    pub canary_routes_bounded_fraction: bool,
    /// §14.2/§14.3: a healthy canary promotes through the *real* L4 → L5 gate with
    /// valid approvals and a rollback-ready bundle.
    pub healthy_canary_promotes_through_gate: bool,
    /// §14.3: the same healthy canary is refused by the gate when approvals are
    /// missing — the canary is a precondition, never a bypass of the gate.
    pub healthy_canary_still_gated: bool,
    /// §14.2/§14.6: a canary whose error budget is exhausted rolls back (under a
    /// declared safety-metric trigger) to the prior known-good release, and does not
    /// promote.
    pub regressing_canary_rolls_back: bool,
    /// §14.2/§14.6: a canary whose correctness gate failed rolls back under the
    /// negative-goal trigger, regardless of the operational signals.
    pub correctness_failure_rolls_back: bool,
    /// Whether every staged-rollout invariant held.
    pub ok: bool,
}

/// Run the staged-rollout drill (PRD §14.2, §21.4): compose shadow eligibility, the
/// shadow stage, the canary stage and the real release gate / deployment controller
/// end to end, and report one boolean per invariant. Deterministic: fixed effect
/// metadata, observations, SLO, gate and clock mean the same inputs always yield the
/// same report.
pub fn rollout_drill() -> RolloutDrillReport {
    // --- §14.2 shadow eligibility: effect substitution requires dry-run support on
    //     every consequential effect.
    let dry_runnable = effect(EffectClass::Write, true, "helpdesk.draft");
    let read_only = effect(EffectClass::Read, false, "helpdesk.lookup");
    let no_dry_run = effect(EffectClass::External, false, "payments.charge");
    let eligible = shadow_eligibility(&[dry_runnable.clone(), read_only.clone()]);
    let ineligible = shadow_eligibility(&[dry_runnable.clone(), no_dry_run]);
    let shadow_requires_dry_run = eligible == ShadowEligibility::Eligible
        && matches!(ineligible, ShadowEligibility::Ineligible { blocking } if blocking == ["payments.charge"]);

    // --- §14.2 shadow run: the candidate carries consequential effects on every
    //     input, yet shadowing commits none of them (all substituted); only the
    //     baseline commits. Parity is perfect, so the candidate proceeds to canary.
    let shadow_obs = shadow_observations();
    let shadow = run_shadow(&ShadowConfig::default(), &shadow_obs);
    // Each of the 30 inputs: candidate would commit 1 effect, baseline commits 1.
    let shadow_commits_no_candidate_effects =
        shadow.candidate_effects_substituted == 30 && shadow.baseline_effects_committed == 30;
    let shadow_parity_proceeds = shadow.decision == ShadowDecision::Proceed
        && shadow.divergences == 0
        && shadow.candidate_new_errors == 0;

    // --- §14.2 shadow divergence: a candidate that newly errors holds.
    let mut diverging = shadow_observations();
    diverging[0].candidate = RunOutcome::ok("reply", 10.0).with_effects(1).failed();
    let shadow_bad = run_shadow(&ShadowConfig::default(), &diverging);
    let shadow_divergence_holds = matches!(shadow_bad.decision, ShadowDecision::Hold { .. })
        && shadow_bad.candidate_new_errors == 1;

    // --- §14.2 canary: a bounded 20% of traffic is routed to the candidate.
    let healthy = canary_observations(false, false); // no errors, no metric drift
    let canary_cfg = canary_config();
    let healthy_report = run_canary(&canary_cfg, &healthy, Correctness::Clean);
    let canary_routes_bounded_fraction = healthy_report.canary_runs > 0
        && healthy_report.canary_runs < healthy_report.total_runs
        && healthy_report.realized_fraction <= canary_cfg.traffic_fraction + 0.1;

    // --- §14.2/§14.3 healthy canary → promote verdict → real L4→L5 gate promotes.
    let promote_verdict_healthy = healthy_report.verdict == CanaryVerdict::Promote;
    let ready = ready_bundle();
    let (keyring, binding, approvals) = valid_approval();
    let clock = entelechy_identity::FixedClock(Some(1_000_060));
    let check = ApprovalCheck {
        approvals: &approvals,
        current_binding: &binding,
        keyring: &keyring,
        clock: &clock,
    };
    let assurance = eligible_report();
    let gate = PromotionGate {
        assurance: &assurance,
        holdout_passed: true,
        negative_goal_clean: true,
        evidence_state: entelechy_artifacts::EvidenceState::Complete,
    };
    let mut dc = staged_controller();
    let promoted = drive_staged_promotion(
        &healthy_report,
        &ready,
        &gate,
        &check,
        &mut dc,
        DeployAuthority {
            can_promote: false,
            can_stop: true,
        },
    );
    let healthy_canary_promotes_through_gate =
        promote_verdict_healthy && matches!(promoted, StagedOutcome::Promoted(_));

    // --- §14.3 the same healthy canary, but with NO approval presented, is refused
    //     by the gate — the canary never bypasses the gate.
    let no_approvals: Vec<entelechy_identity::Approval> = vec![];
    let empty_check = ApprovalCheck {
        approvals: &no_approvals,
        current_binding: &binding,
        keyring: &keyring,
        clock: &clock,
    };
    let mut dc2 = staged_controller();
    let gated = drive_staged_promotion(
        &healthy_report,
        &ready,
        &gate,
        &empty_check,
        &mut dc2,
        DeployAuthority {
            can_promote: false,
            can_stop: true,
        },
    );
    let healthy_canary_still_gated = matches!(
        gated,
        StagedOutcome::GateRefused(PromotionError::MissingValidApproval)
    );

    // --- §14.2/§14.6 a canary whose error budget is exhausted rolls back to the
    //     prior known-good release (SafetyMetricBreach), and does not promote.
    let erroring = canary_observations(true, false); // candidate errors on its slice
    let budget_report = run_canary(&canary_cfg, &erroring, Correctness::Clean);
    let budget_rollback_verdict = matches!(
        budget_report.verdict,
        CanaryVerdict::Rollback {
            trigger: AutoTrigger::SafetyMetricBreach,
            ..
        }
    );
    let mut dc3 = staged_controller();
    let rolled = drive_staged_promotion(
        &budget_report,
        &ready,
        &gate,
        &check,
        &mut dc3,
        DeployAuthority {
            can_promote: false,
            can_stop: true,
        },
    );
    let regressing_canary_rolls_back = budget_report.budget.is_exhausted()
        && budget_rollback_verdict
        && matches!(
            &rolled,
            StagedOutcome::RolledBack { to, trigger }
                if to == "rel-baseline" && *trigger == AutoTrigger::SafetyMetricBreach
        )
        && dc3.active() == Some(&"rel-baseline".to_string());

    // --- §14.2/§14.6 a failed correctness gate rolls back under the negative-goal
    //     trigger, regardless of the (healthy) operational signals.
    let correctness_report = run_canary(&canary_cfg, &healthy, Correctness::Failed);
    let mut dc4 = staged_controller();
    let cf_rolled = drive_staged_promotion(
        &correctness_report,
        &ready,
        &gate,
        &check,
        &mut dc4,
        DeployAuthority {
            can_promote: false,
            can_stop: true,
        },
    );
    let correctness_failure_rolls_back = matches!(
        correctness_report.verdict,
        CanaryVerdict::Rollback {
            trigger: AutoTrigger::NegativeGoalViolation,
            ..
        }
    ) && matches!(
        &cf_rolled,
        StagedOutcome::RolledBack { to, trigger }
            if to == "rel-baseline" && *trigger == AutoTrigger::NegativeGoalViolation
    );

    let ok = shadow_requires_dry_run
        && shadow_commits_no_candidate_effects
        && shadow_parity_proceeds
        && shadow_divergence_holds
        && canary_routes_bounded_fraction
        && healthy_canary_promotes_through_gate
        && healthy_canary_still_gated
        && regressing_canary_rolls_back
        && correctness_failure_rolls_back;

    RolloutDrillReport {
        shadow_requires_dry_run,
        shadow_commits_no_candidate_effects,
        shadow_parity_proceeds,
        shadow_divergence_holds,
        canary_routes_bounded_fraction,
        healthy_canary_promotes_through_gate,
        healthy_canary_still_gated,
        regressing_canary_rolls_back,
        correctness_failure_rolls_back,
        ok,
    }
}

/// An effect metadatum with the given class and dry-run support (other fields fixed).
fn effect(class: EffectClass, dry_run_supported: bool, ns: &str) -> EffectMetadata {
    EffectMetadata {
        class,
        idempotent: false,
        reversible: false,
        dry_run_supported,
        read_back_supported: false,
        attestation: entelechy_ir::AttestationLevel::OperatorAttested,
        operation_key_namespace: ns.into(),
    }
}

/// Thirty shadow inputs on which the candidate agrees with the baseline, each side
/// carrying one consequential effect (so the drill can show the candidate's are all
/// substituted while the baseline's commit).
fn shadow_observations() -> Vec<RolloutObservation> {
    (0..30)
        .map(|i| RolloutObservation {
            run_id: format!("shadow-{i}"),
            baseline: RunOutcome::ok("reply", 10.0).with_effects(1),
            candidate: RunOutcome::ok("reply", 10.0).with_effects(1),
        })
        .collect()
}

/// A hundred canary inputs. `candidate_errors` makes the candidate error on every
/// input (exhausting the slice's error budget); `candidate_drift` shifts the
/// candidate metric far from the baseline (tripping drift).
fn canary_observations(candidate_errors: bool, candidate_drift: bool) -> Vec<RolloutObservation> {
    (0..100)
        .map(|i| {
            let mut candidate =
                RunOutcome::ok("reply", if candidate_drift { 90.0 } else { 10.0 }).with_effects(1);
            if candidate_errors {
                candidate = candidate.failed();
            }
            RolloutObservation {
                run_id: format!("canary-{i}"),
                baseline: RunOutcome::ok("reply", 10.0).with_effects(1),
                candidate,
            }
        })
        .collect()
}

/// A canary config routing 20% of traffic, measured against a 99.9% SLO.
fn canary_config() -> CanaryConfig {
    CanaryConfig {
        traffic_fraction: 0.2,
        slo: ServiceLevelObjective {
            name: "canary tier-1 success".into(),
            window_secs: 3600,
            population: "canary slice".into(),
            statistic: crate::SloStatistic::SuccessRate,
            target: 0.999,
            exclusions: Some(vec![]),
        },
        drift_kind: DriftKind::Quality,
        drift_k: 3.0,
        min_canary_runs: 5,
    }
}

/// A rollback-ready L5 bundle so the RB-2 readiness gate never masks the staged
/// result under test.
fn ready_bundle() -> ReleaseBundle {
    ReleaseBundle {
        design_id: "sha256:d".into(),
        goalspec_id: "sha256:g".into(),
        authority_id: "sha256:a".into(),
        evalcontract_id: "sha256:e".into(),
        safety_case: entelechy_assurance::SafetyCase::default(),
        policy_snapshot_version: 1,
        model_tool_ids: vec!["mock@r1".into()],
        provenance: "rollout-drill".into(),
        rollback_target: Some("rel-baseline".into()),
        no_rollback_rationale: None,
    }
}

/// A release-eligible assurance report (no static violations, default safety case)
/// — the caller builds a clean [`PromotionGate`] by borrowing it.
fn eligible_report() -> entelechy_assurance::AssuranceReport {
    entelechy_assurance::AssuranceReport {
        static_violations: vec![],
        safety_case: entelechy_assurance::SafetyCase::default(),
        policy_snapshot_version: 1,
    }
}

/// A deployment controller with the known-good baseline deployed, then the candidate
/// on top — so a rollback recovers `rel-baseline`. The declared triggers are the two
/// a canary regression maps to.
fn staged_controller() -> DeploymentController {
    let releaser = DeployAuthority {
        can_promote: true,
        can_stop: false,
    };
    let mut dc = DeploymentController::new(vec![
        AutoTrigger::SafetyMetricBreach,
        AutoTrigger::NegativeGoalViolation,
    ]);
    dc.deploy("rel-baseline", releaser)
        .expect("releaser deploys the known-good baseline");
    dc.deploy("rel-candidate", releaser)
        .expect("releaser deploys the canary candidate");
    dc
}

/// A fixed, currently-valid hash-bound approval over the bundle's design (PRD §14.5).
fn valid_approval() -> (
    entelechy_identity::KeyRing,
    entelechy_identity::ApprovalBinding,
    Vec<entelechy_identity::Approval>,
) {
    use std::collections::BTreeMap;
    let mut ring = entelechy_identity::KeyRing::new();
    ring.add_key("k", "s");
    let binding: entelechy_identity::ApprovalBinding =
        BTreeMap::from([("design".to_string(), "sha256:d".to_string())]);
    let approval = entelechy_identity::Approval::sign(
        entelechy_identity::Principal::new("approver", entelechy_identity::PrincipalKind::Approver),
        binding.clone(),
        "k",
        &ring,
        1_000_000,
        Some(2_000_000),
    )
    .expect("signing key present");
    (ring, binding, vec![approval])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_eligibility_fails_closed_on_non_dry_run_effect() {
        // A read-only effect needs no substitution; a dry-runnable write is fine.
        let ok = shadow_eligibility(&[
            effect(EffectClass::Read, false, "lookup"),
            effect(EffectClass::Write, true, "draft"),
        ]);
        assert_eq!(ok, ShadowEligibility::Eligible);
        assert!(ok.is_eligible());

        // A consequential effect without dry-run blocks shadow and is named.
        let blocked = shadow_eligibility(&[
            effect(EffectClass::Write, true, "draft"),
            effect(EffectClass::External, false, "charge"),
            effect(EffectClass::Irreversible, false, "wire"),
        ]);
        assert_eq!(
            blocked,
            ShadowEligibility::Ineligible {
                blocking: vec!["charge".into(), "wire".into()]
            }
        );
        assert!(!blocked.is_eligible());
    }

    #[test]
    fn shadow_substitutes_all_candidate_effects() {
        let obs = shadow_observations();
        let report = run_shadow(&ShadowConfig::default(), &obs);
        // The candidate "would" commit 30 effects; all are substituted, none committed.
        assert_eq!(report.candidate_effects_substituted, 30);
        assert_eq!(report.baseline_effects_committed, 30);
        assert_eq!(report.decision, ShadowDecision::Proceed);
        assert_eq!(report.divergences, 0);
        assert!((report.agreement_rate() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn shadow_holds_on_too_few_samples_new_errors_or_low_agreement() {
        // Too few samples.
        let few = run_shadow(&ShadowConfig::default(), &shadow_observations()[..5]);
        assert!(matches!(few.decision, ShadowDecision::Hold { .. }));

        // A new candidate error.
        let mut errored = shadow_observations();
        errored[0].candidate = RunOutcome::ok("reply", 10.0).failed();
        let r = run_shadow(&ShadowConfig::default(), &errored);
        assert_eq!(r.candidate_new_errors, 1);
        assert!(matches!(r.decision, ShadowDecision::Hold { .. }));

        // Low agreement (half diverge), no new errors.
        let mut diverged = shadow_observations();
        for (i, o) in diverged.iter_mut().enumerate() {
            if i % 2 == 0 {
                o.candidate = RunOutcome::ok("different", 10.0).with_effects(1);
            }
        }
        let r = run_shadow(&ShadowConfig::default(), &diverged);
        assert_eq!(r.candidate_new_errors, 0);
        assert!(r.agreement_rate() < 0.95);
        assert!(matches!(r.decision, ShadowDecision::Hold { .. }));
    }

    #[test]
    fn canary_routes_a_bounded_deterministic_fraction() {
        let cfg = canary_config();
        let obs = canary_observations(false, false);
        let a = run_canary(&cfg, &obs, Correctness::Clean);
        let b = run_canary(&cfg, &obs, Correctness::Clean);
        // Deterministic routing.
        assert_eq!(a.canary_runs, b.canary_runs);
        // Bounded: some traffic, far from all, near the 20% target.
        assert!(a.canary_runs > 0 && a.canary_runs < a.total_runs);
        assert!(a.realized_fraction <= cfg.traffic_fraction + 0.1);
        assert_eq!(a.canary_runs + a.baseline_runs, a.total_runs);
    }

    #[test]
    fn healthy_canary_promotes_but_only_through_the_gate() {
        let cfg = canary_config();
        let report = run_canary(&cfg, &canary_observations(false, false), Correctness::Clean);
        assert_eq!(report.verdict, CanaryVerdict::Promote);
        assert!(!report.budget.is_exhausted());
        assert!(report.drift.is_none());

        let ready = ready_bundle();
        let assurance = eligible_report();
        let gate = PromotionGate {
            assurance: &assurance,
            holdout_passed: true,
            negative_goal_clean: true,
            evidence_state: entelechy_artifacts::EvidenceState::Complete,
        };
        let (ring, binding, appr) = valid_approval();
        let clock = entelechy_identity::FixedClock(Some(1_000_060));
        let check = ApprovalCheck {
            approvals: &appr,
            current_binding: &binding,
            keyring: &ring,
            clock: &clock,
        };
        let stopper = DeployAuthority {
            can_promote: false,
            can_stop: true,
        };

        // With valid approvals, the healthy canary promotes through the real gate.
        let mut dc = staged_controller();
        let out = drive_staged_promotion(&report, &ready, &gate, &check, &mut dc, stopper);
        assert!(matches!(out, StagedOutcome::Promoted(_)));

        // Without approvals, the SAME healthy canary is refused by the gate — the
        // canary is a precondition, never a bypass.
        let none: Vec<entelechy_identity::Approval> = vec![];
        let empty = ApprovalCheck {
            approvals: &none,
            current_binding: &binding,
            keyring: &ring,
            clock: &clock,
        };
        let mut dc2 = staged_controller();
        let out = drive_staged_promotion(&report, &ready, &gate, &empty, &mut dc2, stopper);
        assert_eq!(
            out,
            StagedOutcome::GateRefused(PromotionError::MissingValidApproval)
        );
    }

    #[test]
    fn regressing_canary_rolls_back_and_does_not_promote() {
        let cfg = canary_config();
        // Budget exhaustion: the candidate errors on its whole slice.
        let report = run_canary(&cfg, &canary_observations(true, false), Correctness::Clean);
        assert!(report.budget.is_exhausted());
        assert!(matches!(
            report.verdict,
            CanaryVerdict::Rollback {
                trigger: AutoTrigger::SafetyMetricBreach,
                ..
            }
        ));

        let ready = ready_bundle();
        let assurance = eligible_report();
        let gate = PromotionGate {
            assurance: &assurance,
            holdout_passed: true,
            negative_goal_clean: true,
            evidence_state: entelechy_artifacts::EvidenceState::Complete,
        };
        let (ring, binding, appr) = valid_approval();
        let clock = entelechy_identity::FixedClock(Some(1_000_060));
        let check = ApprovalCheck {
            approvals: &appr,
            current_binding: &binding,
            keyring: &ring,
            clock: &clock,
        };
        let mut dc = staged_controller();
        let out = drive_staged_promotion(
            &report,
            &ready,
            &gate,
            &check,
            &mut dc,
            DeployAuthority {
                can_promote: false,
                can_stop: true,
            },
        );
        match out {
            StagedOutcome::RolledBack { to, trigger } => {
                assert_eq!(to, "rel-baseline");
                assert_eq!(trigger, AutoTrigger::SafetyMetricBreach);
            }
            other => panic!("expected a rollback, got {other:?}"),
        }
        assert_eq!(dc.active(), Some(&"rel-baseline".to_string()));
    }

    #[test]
    fn metric_drift_in_the_canary_slice_rolls_back() {
        let cfg = canary_config();
        // No errors (budget intact) but the candidate metric drifts far from baseline.
        let report = run_canary(&cfg, &canary_observations(false, true), Correctness::Clean);
        assert!(!report.budget.is_exhausted());
        assert!(report.drift.is_some());
        assert!(matches!(
            report.verdict,
            CanaryVerdict::Rollback {
                trigger: AutoTrigger::SafetyMetricBreach,
                ..
            }
        ));
    }

    #[test]
    fn correctness_failure_rolls_back_regardless_of_operational_signals() {
        let cfg = canary_config();
        // Operationally healthy slice, but the correctness gate failed.
        let report = run_canary(
            &cfg,
            &canary_observations(false, false),
            Correctness::Failed,
        );
        assert!(!report.budget.is_exhausted());
        assert!(matches!(
            report.verdict,
            CanaryVerdict::Rollback {
                trigger: AutoTrigger::NegativeGoalViolation,
                ..
            }
        ));
    }

    #[test]
    fn too_few_canary_runs_holds() {
        let mut cfg = canary_config();
        cfg.min_canary_runs = 1_000; // more than any slice of 100 inputs will reach
        let report = run_canary(&cfg, &canary_observations(false, false), Correctness::Clean);
        assert!(matches!(report.verdict, CanaryVerdict::Hold { .. }));
    }

    #[test]
    fn verdicts_serialize_as_tagged() {
        let v = CanaryVerdict::Rollback {
            trigger: AutoTrigger::SafetyMetricBreach,
            reason: "budget".into(),
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains(r#""verdict":"rollback""#), "{json}");
        let back: CanaryVerdict = serde_json::from_str(&json).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn rollout_drill_satisfies_every_invariant() {
        let r = rollout_drill();
        assert!(r.ok, "every staged-rollout invariant must hold: {r:?}");
        assert!(r.shadow_requires_dry_run);
        assert!(r.shadow_commits_no_candidate_effects);
        assert!(r.shadow_parity_proceeds);
        assert!(r.shadow_divergence_holds);
        assert!(r.canary_routes_bounded_fraction);
        assert!(r.healthy_canary_promotes_through_gate);
        assert!(r.healthy_canary_still_gated);
        assert!(r.regressing_canary_rolls_back);
        assert!(r.correctness_failure_rolls_back);
    }

    #[test]
    fn the_drill_is_deterministic() {
        assert_eq!(rollout_drill(), rollout_drill());
    }
}
