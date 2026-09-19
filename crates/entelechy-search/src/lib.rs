//! Study driver: candidate accounting, rolling-validation acceptance and
//! stopping rules.
//!
//! PRD v11 references: section 11.7 (search strategies), 11.8 (stopping), 21.1
//! (candidate accounting & stopping), EV-13 (rolling validation), 9.4 (adaptive
//! overfitting controls).
//!
//! The driver operates on per-task pass/fail outcomes and design content hashes,
//! so it stays decoupled from the IR and the runtime. A real study wires the
//! Design Repair Engine (`entelechy-design`) to propose candidates and the
//! runtime + `entelechy-eval` to produce outcomes.
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

use entelechy_bench::{StoppingRule, StudyPlan};
use entelechy_eval::paired_comparison;

/// The decision for a proposed mutation (PRD 11.2 result field, EV-13).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    /// Won on tune and confirmed on rolling validation: keep it.
    Accepted,
    /// Did not win on tune: reject.
    Rejected,
    /// Won on tune but not confirmed on validation: revert, record inconclusive
    /// (PRD 9.4 rolling validation, EV-13).
    Inconclusive,
}

/// Apply the rolling-validation rule (EV-13, PRD 9.4): a change is accepted only
/// if it wins on tune *and* is confirmed on fresh validation tasks.
pub fn rolling_validation_decision(
    tune_baseline: &[bool],
    tune_candidate: &[bool],
    val_baseline: &[bool],
    val_candidate: &[bool],
    seed: u64,
) -> Decision {
    let tune = paired_comparison(tune_baseline, tune_candidate, seed);
    if !tune.improves() {
        return Decision::Rejected;
    }
    let val = paired_comparison(val_baseline, val_candidate, seed ^ 0xD1CE);
    if val.improves() {
        Decision::Accepted
    } else {
        Decision::Inconclusive
    }
}

/// Why a study stopped (PRD 11.8, 21.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopReason {
    /// Candidate budget exhausted (PRD 21.1 — includes failed/invalid candidates).
    BudgetExhausted,
    /// The non-binding futility check at half budget fired (PRD 21.1, Q23).
    Futility,
    /// A human requested a stop.
    HumanAbort,
}

/// A study candidate identified by its design content hash.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Design content hash (from `entelechy-artifacts`).
    pub design_hash: String,
    /// Validation success rate at acceptance time (fraction).
    pub validation_success: f64,
}

/// Errors from the study driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StudyError {
    /// No budget remains; the candidate was not accounted.
    BudgetExhausted,
}

/// A study: candidate accounting, an accepted-candidate archive and stopping
/// (PRD 21.1). Every *evaluated* candidate consumes budget, including failed,
/// invalid and abandoned ones — search cannot hide unsuccessful trials.
pub struct Study {
    candidate_budget: u32,
    consumed: u32,
    futility_used: bool,
    accepted: Vec<Candidate>,
    stopped: Option<StopReason>,
}

impl Study {
    /// Start a study from a signed StudyPlan.
    pub fn from_plan(plan: &StudyPlan) -> Self {
        Self {
            candidate_budget: plan.candidate_budget(),
            consumed: 0,
            futility_used: false,
            accepted: Vec::new(),
            stopped: None,
        }
    }

    /// Account one evaluated candidate against the budget (PRD 21.1). Call this
    /// for *every* candidate the study evaluates, before recording its result.
    pub fn account_candidate(&mut self) -> Result<(), StudyError> {
        if self.consumed >= self.candidate_budget {
            self.stopped.get_or_insert(StopReason::BudgetExhausted);
            return Err(StudyError::BudgetExhausted);
        }
        self.consumed += 1;
        Ok(())
    }

    /// Record an accepted candidate in the archive.
    pub fn accept(&mut self, candidate: Candidate) {
        self.accepted.push(candidate);
    }

    /// Number of candidates evaluated so far.
    pub fn consumed(&self) -> u32 {
        self.consumed
    }

    /// Remaining candidate budget.
    pub fn remaining(&self) -> u32 {
        self.candidate_budget.saturating_sub(self.consumed)
    }

    /// The accepted-candidate archive.
    pub fn archive(&self) -> &[Candidate] {
        &self.accepted
    }

    /// The best accepted candidate by validation success (PRD 5.7 selection is
    /// richer; this is the Phase 0 single-metric proxy).
    pub fn best(&self) -> Option<&Candidate> {
        self.accepted
            .iter()
            .max_by(|a, b| a.validation_success.partial_cmp(&b.validation_success).unwrap())
    }

    /// The single non-binding futility check at half budget (PRD 21.1). Returns
    /// true once (and records the stop) if invoked with `improving == false`
    /// after at least half the budget is spent. Never fires for success — early
    /// stopping for success is disallowed in Phase 0 (interim looks at a
    /// fixed-sample analysis inflate false positives).
    pub fn futility_check(&mut self, improving: bool) -> bool {
        if self.futility_used {
            return false;
        }
        if self.consumed * 2 >= self.candidate_budget {
            self.futility_used = true;
            if !improving {
                self.stopped.get_or_insert(StopReason::Futility);
                return true;
            }
        }
        false
    }

    /// Request a human stop.
    pub fn abort(&mut self) {
        self.stopped.get_or_insert(StopReason::HumanAbort);
    }

    /// Whether the study has stopped, and why.
    pub fn stop_reason(&self) -> Option<StopReason> {
        if self.consumed >= self.candidate_budget {
            return Some(self.stopped.unwrap_or(StopReason::BudgetExhausted));
        }
        self.stopped
    }

    /// The stopping rule variant this study was configured with (informational).
    pub fn describe_rule(plan: &StudyPlan) -> &'static str {
        match plan.stopping {
            StoppingRule::Phase0Default { .. } => "phase0: budget cap + non-binding futility at half",
            StoppingRule::GroupSequential { .. } => "group-sequential: O'Brien-Fleming alpha-spending",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_bench::{StoppingRule, StudyPlan};
    use entelechy_eval::SplitPolicy;

    fn plan(budget: u32) -> StudyPlan {
        StudyPlan {
            revision: 1,
            manifest_id: "sha256:aa".into(),
            baseline_id: "sha256:bb".into(),
            primary_metric: "task_success".into(),
            splits: SplitPolicy::default(),
            mde_validation_pp: 17.0,
            mde_holdout_pp: 12.0,
            stopping: StoppingRule::Phase0Default { candidate_budget: budget },
            analysis: "paired bootstrap".into(),
            frozen_models: vec!["mock".into()],
            signed_by: "owner".into(),
        }
    }

    #[test]
    fn accepts_only_when_confirmed_on_validation() {
        let tune_b = vec![false; 40];
        let tune_c = vec![true; 40];
        let val_b = vec![false; 20];
        let val_c = vec![true; 20];
        assert_eq!(
            rolling_validation_decision(&tune_b, &tune_c, &val_b, &val_c, 1),
            Decision::Accepted
        );
    }

    #[test]
    fn tune_win_without_validation_is_inconclusive() {
        let tune_b = vec![false; 40];
        let tune_c = vec![true; 40];
        // No improvement on validation: identical outcomes.
        let val = vec![true, false, true, true, false, true, false, true, true, false];
        assert_eq!(
            rolling_validation_decision(&tune_b, &tune_c, &val, &val, 1),
            Decision::Inconclusive
        );
    }

    #[test]
    fn no_tune_win_is_rejected() {
        let x = vec![true, false, true, false, true, false];
        assert_eq!(rolling_validation_decision(&x, &x, &x, &x, 1), Decision::Rejected);
    }

    #[test]
    fn candidate_accounting_and_budget_stop() {
        let mut study = Study::from_plan(&plan(3));
        assert!(study.account_candidate().is_ok());
        assert!(study.account_candidate().is_ok());
        assert!(study.account_candidate().is_ok());
        assert_eq!(study.account_candidate(), Err(StudyError::BudgetExhausted));
        assert_eq!(study.stop_reason(), Some(StopReason::BudgetExhausted));
        assert_eq!(study.remaining(), 0);
    }

    #[test]
    fn futility_fires_once_at_half_when_not_improving() {
        let mut study = Study::from_plan(&plan(10));
        for _ in 0..5 {
            study.account_candidate().unwrap();
        }
        assert!(study.futility_check(false));
        assert_eq!(study.stop_reason(), Some(StopReason::Futility));
        // Only once.
        assert!(!study.futility_check(false));
    }

    #[test]
    fn best_tracks_highest_validation_success() {
        let mut study = Study::from_plan(&plan(10));
        study.accept(Candidate { design_hash: "a".into(), validation_success: 0.6 });
        study.accept(Candidate { design_hash: "b".into(), validation_success: 0.8 });
        assert_eq!(study.best().unwrap().design_hash, "b");
    }
}
