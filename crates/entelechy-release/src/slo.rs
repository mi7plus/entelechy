//! Service-level objectives, error budgets, and the SLO drill (PRD §18.1, SL-1).
//!
//! SL-1 has two halves, and this module mechanizes both:
//!
//! - **Every L5 SLO states window, population, statistic/error budget and
//!   exclusions.** A [`ServiceLevelObjective`] carries exactly those four, and
//!   [`ServiceLevelObjective::defects`] fails closed on any that is missing or
//!   malformed, so an under-specified objective can never be declared at L5.
//! - **Correctness gates sit outside error budgets.** The error budget
//!   ([`ErrorBudgetAccount`]) accounts only availability/latency *SLO events* — there
//!   is deliberately no path to charge a correctness-gate outcome to it — and
//!   [`release_health`] evaluates the [`Correctness`] verdict (derived from the real
//!   [`crate::PromotionGate`]) *first and independently* of the budget, so remaining
//!   budget can never buy back a correctness failure and a budget breach is never a
//!   correctness block. The promotion gate itself takes no budget input, which is the
//!   structural statement of the same separation.
//!
//! [`slo_drill`] is the operational drill those requirements call for: it composes
//! the model and the real promotion gate end to end and reports one boolean per
//! SL-1 invariant, so an operator can run it (and persist the report) as a release
//! exercise, mirroring the DR-1, RB and AU drills.

use serde::{Deserialize, Serialize};

use crate::{
    promote, ApprovalCheck, MaturityLevel, PromotionError, PromotionGate, PromotionOverride,
    ReleaseBundle,
};

/// The statistic an SLO measures (PRD §18.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SloStatistic {
    /// Fraction of events in the population that meet the objective
    /// (availability / success rate).
    SuccessRate,
    /// A latency percentile that must stay at or below a threshold (e.g. p99 ≤ N ms).
    LatencyPercentile {
        /// The percentile measured (1..=100).
        percentile: u8,
        /// The threshold in milliseconds.
        threshold_ms: u64,
    },
}

/// A way an L5 SLO declaration is not well-formed (SL-1). Each names one of the four
/// things SL-1 requires an L5 SLO to state, or a malformed value of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SloDefect {
    /// No measurement window stated (`window_secs == 0`).
    MissingWindow,
    /// No population stated (empty).
    MissingPopulation,
    /// The target (and therefore the error budget) is not a fraction in `(0, 1)`.
    InvalidTarget,
    /// A latency percentile outside `1..=100`.
    InvalidPercentile,
    /// Exclusions were not stated at all (SL-1 requires them declared — an explicit
    /// empty set is a decision, `None` is an omission).
    MissingExclusions,
}

/// A declared service-level objective for a production (L5) release (PRD §18.1).
///
/// SL-1 requires an L5 SLO to *state* all four of: a measurement `window`, the
/// `population` it covers, the `statistic` plus its `target` (from which the error
/// budget is derived), and the `exclusions`. `exclusions` is `None` when exclusions
/// were never stated (ill-formed); `Some(_)` — possibly empty — is an explicit
/// declaration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceLevelObjective {
    /// Human-readable objective name.
    pub name: String,
    /// Rolling measurement window in seconds (must be `> 0`).
    pub window_secs: u64,
    /// The population measured (must be non-empty), e.g. `"tier-1 tickets"`.
    pub population: String,
    /// The statistic measured.
    pub statistic: SloStatistic,
    /// The target as a fraction in `(0, 1)` — e.g. `0.999`. The error budget is
    /// `1 - target`.
    pub target: f64,
    /// Declared exclusions (maintenance windows, upstream outages, …). `None` means
    /// exclusions were not stated at all.
    pub exclusions: Option<Vec<String>>,
}

impl ServiceLevelObjective {
    /// Every way this SLO is not well-formed for an L5 declaration (SL-1). An empty
    /// result means it states all four required things with sane values.
    pub fn defects(&self) -> Vec<SloDefect> {
        let mut defects = Vec::new();
        if self.window_secs == 0 {
            defects.push(SloDefect::MissingWindow);
        }
        if self.population.trim().is_empty() {
            defects.push(SloDefect::MissingPopulation);
        }
        if !(self.target > 0.0 && self.target < 1.0) {
            defects.push(SloDefect::InvalidTarget);
        }
        if let SloStatistic::LatencyPercentile { percentile, .. } = self.statistic {
            if !(1..=100).contains(&percentile) {
                defects.push(SloDefect::InvalidPercentile);
            }
        }
        if self.exclusions.is_none() {
            defects.push(SloDefect::MissingExclusions);
        }
        defects
    }

    /// Whether this SLO is well-formed for an L5 production declaration (SL-1): it
    /// states window, population, statistic/target and exclusions, all with sane
    /// values.
    pub fn is_well_formed_for_l5(&self) -> bool {
        self.defects().is_empty()
    }

    /// The error budget: the fraction of the population allowed to miss the objective
    /// over the window (`1 - target`), clamped to `[0, 1]`. Only meaningful once the
    /// SLO is well-formed (see [`defects`](Self::defects)).
    pub fn error_budget(&self) -> f64 {
        (1.0 - self.target).clamp(0.0, 1.0)
    }
}

/// The error-budget accounting for an SLO over its window (PRD §18.1).
///
/// Only *SLO events* — availability/latency outcomes in the population — are counted
/// here. A correctness-gate outcome is never an SLO event (SL-1: correctness sits
/// outside the error budget), so there is deliberately no method to charge one to
/// this account.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorBudgetAccount {
    /// Total SLO events observed in the window.
    pub total_events: u64,
    /// Events that missed the objective (the "bad" events).
    pub bad_events: u64,
    /// The allowed error budget as a fraction (`1 - target`).
    pub budget_fraction: f64,
}

impl ErrorBudgetAccount {
    /// Open an empty account for an SLO's error budget.
    pub fn for_slo(slo: &ServiceLevelObjective) -> Self {
        Self {
            total_events: 0,
            bad_events: 0,
            budget_fraction: slo.error_budget(),
        }
    }

    /// Record `bad` of `count` SLO events observed in the window. `bad` is clamped to
    /// `count` (there can be no more bad events than events).
    pub fn observe(&mut self, count: u64, bad: u64) {
        self.total_events = self.total_events.saturating_add(count);
        self.bad_events = self.bad_events.saturating_add(bad.min(count));
    }

    /// The number of bad events the budget permits over the observed volume.
    pub fn budgeted_bad(&self) -> f64 {
        self.total_events as f64 * self.budget_fraction
    }

    /// Whether the error budget is exhausted: more bad events than the budget permits
    /// over the observed volume. With no observed events nothing is spent.
    pub fn is_exhausted(&self) -> bool {
        self.bad_events as f64 > self.budgeted_bad()
    }

    /// Fraction of the error budget remaining (`1.0` = untouched, `0.0` = exhausted).
    /// With no observed events the full budget remains.
    pub fn remaining_fraction(&self) -> f64 {
        let budgeted = self.budgeted_bad();
        if budgeted <= 0.0 {
            // No budgeted headroom over the observed volume: any bad event exhausts.
            if self.bad_events == 0 {
                1.0
            } else {
                0.0
            }
        } else {
            (1.0 - self.bad_events as f64 / budgeted).clamp(0.0, 1.0)
        }
    }
}

/// The correctness verdict — the outcome of the hard correctness gates (assurance
/// eligibility, holdout regression, negative goals) that SL-1 keeps *outside* the
/// error budget. Derived from the same [`PromotionGate`] the real promotion uses, so
/// the drill speaks about the actual correctness gates rather than a parallel notion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Correctness {
    /// Every correctness gate passed.
    Clean,
    /// A correctness gate failed.
    Failed,
}

impl Correctness {
    /// Derive the correctness verdict from the real promotion gate (PRD §14.3): the
    /// correctness gates are clean iff assurance is release-eligible, the holdout did
    /// not regress and negative goals are clean. These are exactly the gates SL-1
    /// keeps outside the error budget.
    pub fn from_gate(gate: &PromotionGate) -> Self {
        if gate.assurance.release_eligible() && gate.holdout_passed && gate.negative_goal_clean {
            Correctness::Clean
        } else {
            Correctness::Failed
        }
    }
}

/// The combined operational + correctness state of a production release (SL-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseHealth {
    /// Correctness clean and the error budget intact.
    Healthy,
    /// Correctness clean but the error budget is exhausted — an operational condition
    /// (freeze changes / page), **not** a correctness block.
    BudgetExhausted,
    /// A correctness gate failed — blocked regardless of remaining error budget
    /// (SL-1: correctness sits outside the budget).
    CorrectnessBlocked,
}

/// Combine the correctness verdict and the error-budget state into a release-health
/// decision (SL-1). Correctness is evaluated *first* and is never offset by remaining
/// budget: a correctness failure blocks whatever the budget says, and an exhausted
/// budget with clean correctness is an operational freeze, never a correctness block.
pub fn release_health(correctness: Correctness, budget: &ErrorBudgetAccount) -> ReleaseHealth {
    match correctness {
        Correctness::Failed => ReleaseHealth::CorrectnessBlocked,
        Correctness::Clean if budget.is_exhausted() => ReleaseHealth::BudgetExhausted,
        Correctness::Clean => ReleaseHealth::Healthy,
    }
}

/// The outcome of the SLO drill (PRD §18.1, SL-1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SloDrillReport {
    /// SL-1: a fully-declared L5 SLO (window, population, statistic/target,
    /// exclusions) is well-formed.
    pub wellformed_slo_accepted: bool,
    /// SL-1: an L5 SLO missing any required field is rejected (fail closed).
    pub incomplete_slo_rejected: bool,
    /// The defects detected on the deliberately under-specified SLO (for the
    /// operator) — one per omitted/malformed field.
    pub detected_defects: Vec<SloDefect>,
    /// SL-1: the error budget equals `1 - target` and the account tracks spend over
    /// observed events (one bad event within budget is not exhaustion; one beyond is).
    pub error_budget_derived: bool,
    /// SL-1: with the error budget fully intact, a correctness-gate failure still
    /// blocks the release (remaining budget never buys back correctness).
    pub correctness_blocks_despite_full_budget: bool,
    /// SL-1: an exhausted error budget with clean correctness is an operational
    /// freeze, not a correctness block (the separation holds in both directions).
    pub budget_breach_is_operational: bool,
    /// SL-1: the real L5 promotion gate refuses a failed correctness gate and takes
    /// no error-budget input — the only way past is a recorded [`PromotionOverride`]
    /// (a human-approved exception), never spending error budget.
    pub correctness_escape_is_explicit_override: bool,
    /// Whether every SL-1 invariant held.
    pub ok: bool,
}

/// Run the SLO drill (PRD §18.1, SL-1). Deterministic: fixed objectives, event
/// counts, gate and clock mean the same inputs always yield the same report.
pub fn slo_drill() -> SloDrillReport {
    // --- SL-1 (declaration): a fully-stated L5 SLO is well-formed; one missing any
    //     of the four required things is rejected, each omission named.
    let wellformed = ServiceLevelObjective {
        name: "tier-1 resolution".into(),
        window_secs: 28 * 24 * 60 * 60, // 28-day rolling window
        population: "tier-1 tickets".into(),
        statistic: SloStatistic::SuccessRate,
        target: 0.999,
        exclusions: Some(vec!["declared maintenance windows".into()]),
    };
    let wellformed_slo_accepted = wellformed.is_well_formed_for_l5();

    // Under-specified on every axis: no window, empty population, out-of-range target
    // (so no error budget), and exclusions never stated.
    let incomplete = ServiceLevelObjective {
        name: "under-specified".into(),
        window_secs: 0,
        population: "   ".into(),
        statistic: SloStatistic::SuccessRate,
        target: 1.0,
        exclusions: None,
    };
    let detected_defects = incomplete.defects();
    let incomplete_slo_rejected = !incomplete.is_well_formed_for_l5()
        && detected_defects.contains(&SloDefect::MissingWindow)
        && detected_defects.contains(&SloDefect::MissingPopulation)
        && detected_defects.contains(&SloDefect::InvalidTarget)
        && detected_defects.contains(&SloDefect::MissingExclusions);

    // --- SL-1 (error budget): budget == 1 - target; spend tracked over events.
    let budget_matches = (wellformed.error_budget() - 0.001).abs() < 1e-9;
    // 0.1% budget over 1000 events = 1 permitted bad event.
    let mut within = ErrorBudgetAccount::for_slo(&wellformed);
    within.observe(1_000, 1);
    let mut beyond = ErrorBudgetAccount::for_slo(&wellformed);
    beyond.observe(1_000, 2);
    let error_budget_derived = budget_matches && !within.is_exhausted() && beyond.is_exhausted();

    // --- SL-1 (correctness outside budget): build the real promotion gate both ways.
    let eligible = eligible_report();
    let clean_gate = PromotionGate {
        assurance: &eligible,
        holdout_passed: true,
        negative_goal_clean: true,
    };
    // A failed *correctness* gate: a negative-goal violation (a hard correctness gate),
    // nothing to do with availability.
    let failed_gate = PromotionGate {
        assurance: &eligible,
        holdout_passed: true,
        negative_goal_clean: false,
    };

    // Full budget remaining (no bad events observed) yet correctness failed → blocked.
    let full_budget = ErrorBudgetAccount::for_slo(&wellformed);
    let correctness_blocks_despite_full_budget = !full_budget.is_exhausted()
        && release_health(Correctness::from_gate(&failed_gate), &full_budget)
            == ReleaseHealth::CorrectnessBlocked;

    // Exhausted budget but correctness clean → operational freeze, not a correctness block.
    let mut spent = ErrorBudgetAccount::for_slo(&wellformed);
    spent.observe(1_000, 50); // far beyond the 1-event budget
    let budget_breach_is_operational = spent.is_exhausted()
        && release_health(Correctness::from_gate(&clean_gate), &spent)
            == ReleaseHealth::BudgetExhausted;

    // --- SL-1 (escape hatch): the real L5 promotion gate refuses the failed
    //     correctness gate and takes no budget input; only a recorded override admits
    //     it. Error budget is never an argument to the decision.
    let (keyring, binding, approvals) = valid_approval();
    let clock = entelechy_identity::FixedClock(Some(1_000_060));
    let check = ApprovalCheck {
        approvals: &approvals,
        current_binding: &binding,
        keyring: &keyring,
        clock: &clock,
    };
    let ready = ready_bundle();
    let promote_l5 = |gate: &PromotionGate, over: Option<PromotionOverride>| {
        promote(
            &ready,
            MaturityLevel::L4,
            MaturityLevel::L5,
            gate,
            &check,
            over,
        )
    };
    let failed_refused = matches!(
        promote_l5(&failed_gate, None),
        Err(PromotionError::NegativeGoalViolation)
    );
    let override_admits = promote_l5(
        &failed_gate,
        Some(PromotionOverride {
            failed_criterion: "negative-goal gate".into(),
            rationale: "accepted risk for the incident hotfix; tracked".into(),
            approver: "release-owner".into(),
            expires_at: 2_000_000,
        }),
    )
    .is_ok();
    // The clean gate promotes with no override and no budget input at all.
    let clean_promotes = promote_l5(&clean_gate, None).is_ok();
    let correctness_escape_is_explicit_override =
        failed_refused && override_admits && clean_promotes;

    let ok = wellformed_slo_accepted
        && incomplete_slo_rejected
        && error_budget_derived
        && correctness_blocks_despite_full_budget
        && budget_breach_is_operational
        && correctness_escape_is_explicit_override;

    SloDrillReport {
        wellformed_slo_accepted,
        incomplete_slo_rejected,
        detected_defects,
        error_budget_derived,
        correctness_blocks_despite_full_budget,
        budget_breach_is_operational,
        correctness_escape_is_explicit_override,
        ok,
    }
}

/// A release-eligible assurance report (no static violations, default safety case).
fn eligible_report() -> entelechy_assurance::AssuranceReport {
    entelechy_assurance::AssuranceReport {
        static_violations: vec![],
        safety_case: entelechy_assurance::SafetyCase::default(),
        policy_snapshot_version: 1,
    }
}

/// A rollback-ready L5 bundle so the RB-2 readiness gate never masks the correctness
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
        provenance: "slo-drill".into(),
        rollback_target: Some("rel-baseline".into()),
        no_rollback_rationale: None,
    }
}

/// A fixed, currently-valid hash-bound approval over the bundle's design (PRD 14.5).
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
    fn slo_drill_satisfies_sl1() {
        let r = slo_drill();
        assert!(r.ok, "every SL-1 invariant must hold: {r:?}");
        assert!(r.wellformed_slo_accepted);
        assert!(r.incomplete_slo_rejected);
        assert_eq!(r.detected_defects.len(), 4);
        assert!(r.error_budget_derived);
        assert!(r.correctness_blocks_despite_full_budget);
        assert!(r.budget_breach_is_operational);
        assert!(r.correctness_escape_is_explicit_override);
    }

    #[test]
    fn the_drill_is_deterministic() {
        assert_eq!(slo_drill(), slo_drill());
    }

    #[test]
    fn an_l5_slo_must_state_all_four() {
        // Each omission is caught independently.
        let base = ServiceLevelObjective {
            name: "x".into(),
            window_secs: 3600,
            population: "p".into(),
            statistic: SloStatistic::SuccessRate,
            target: 0.99,
            exclusions: Some(vec![]),
        };
        assert!(base.is_well_formed_for_l5(), "the base SLO is well-formed");

        let no_window = ServiceLevelObjective {
            window_secs: 0,
            ..base.clone()
        };
        assert_eq!(no_window.defects(), vec![SloDefect::MissingWindow]);

        let no_pop = ServiceLevelObjective {
            population: String::new(),
            ..base.clone()
        };
        assert_eq!(no_pop.defects(), vec![SloDefect::MissingPopulation]);

        let no_budget = ServiceLevelObjective {
            target: 1.0, // no error budget
            ..base.clone()
        };
        assert_eq!(no_budget.defects(), vec![SloDefect::InvalidTarget]);

        let no_exclusions = ServiceLevelObjective {
            exclusions: None,
            ..base.clone()
        };
        assert_eq!(no_exclusions.defects(), vec![SloDefect::MissingExclusions]);

        let bad_pct = ServiceLevelObjective {
            statistic: SloStatistic::LatencyPercentile {
                percentile: 0,
                threshold_ms: 200,
            },
            ..base.clone()
        };
        assert_eq!(bad_pct.defects(), vec![SloDefect::InvalidPercentile]);
    }

    #[test]
    fn error_budget_is_one_minus_target() {
        let slo = ServiceLevelObjective {
            name: "x".into(),
            window_secs: 3600,
            population: "p".into(),
            statistic: SloStatistic::SuccessRate,
            target: 0.99, // 1% budget => 10 bad per 1000 events
            exclusions: Some(vec![]),
        };
        assert!((slo.error_budget() - 0.01).abs() < 1e-9);
        let mut acct = ErrorBudgetAccount::for_slo(&slo);
        acct.observe(1_000, 5); // well within the 10-event budget
        assert!(!acct.is_exhausted());
        assert!(acct.remaining_fraction() > 0.0);
        acct.observe(1_000, 20); // 2000 events, 25 bad; budget is 20 => exhausted
        assert!(acct.is_exhausted());
        assert!((acct.remaining_fraction() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn correctness_is_independent_of_budget_both_ways() {
        let slo = ServiceLevelObjective {
            name: "x".into(),
            window_secs: 3600,
            population: "p".into(),
            statistic: SloStatistic::SuccessRate,
            target: 0.99,
            exclusions: Some(vec![]),
        };
        // Full budget, failed correctness → blocked.
        let full = ErrorBudgetAccount::for_slo(&slo);
        assert_eq!(
            release_health(Correctness::Failed, &full),
            ReleaseHealth::CorrectnessBlocked
        );
        // Exhausted budget, clean correctness → operational, not a correctness block.
        let mut spent = ErrorBudgetAccount::for_slo(&slo);
        spent.observe(100, 100);
        assert!(spent.is_exhausted());
        assert_eq!(
            release_health(Correctness::Clean, &spent),
            ReleaseHealth::BudgetExhausted
        );
    }
}
