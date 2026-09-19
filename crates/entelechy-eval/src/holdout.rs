//! Holdout vault and the evaluation leakage firewall (PRD EV-14, EL-1, 9.6).
//!
//! Holdout content is reachable only through an audited release-gate API that
//! returns pass/fail with an interval, never per-task results (EV-14). The
//! firewall is enforced by interfaces and identities, not prompt instructions
//! (9.6): design/search identities cannot call the gate at all, cannot enumerate
//! holdout task IDs and cannot read holdout inputs. Every query is recorded with
//! caller identity, candidate hash, contract version, split, budget consumption
//! and returned information class (9.6). Gate responses are coarse; once the
//! query budget is partly consumed the gate widens its interval and eventually
//! refuses, following reusable-holdout techniques (9.6).

use serde::{Deserialize, Serialize};

use crate::contract::EvalContract;
use crate::stats::paired_comparison;
use crate::task::{Split, Task};

/// The trust plane a caller belongs to (PRD 5.8, 9.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Plane {
    /// Control-plane design/search identity. May NOT touch the holdout (9.6).
    DesignSearch,
    /// Assurance service. The only plane allowed to query the gate (14.1).
    Assurance,
    /// Auditor. May read the audit log but not query the gate.
    Auditor,
}

/// A caller identity (PRD 5.8). Every query records it (9.6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallerIdentity {
    /// Principal id.
    pub id: String,
    /// The plane this identity belongs to.
    pub plane: Plane,
}

/// The class of information a gate response returned (9.6 audit field).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InformationClass {
    /// A coarse aggregate gate result (pass/fail + interval).
    AggregateGateResult,
    /// The query was refused (budget exhausted or unauthorized).
    Refused,
}

/// A coarse gate response (EV-14): pass/fail with an interval, never per-task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GateResponse {
    /// The candidate meets the contract on the holdout.
    Pass {
        /// Interval on the improvement (percentage points), lower bound.
        ci_low_pp: f64,
        /// Interval upper bound (percentage points).
        ci_high_pp: f64,
    },
    /// The candidate does not meet the contract.
    Fail {
        /// Interval lower bound (percentage points).
        ci_low_pp: f64,
        /// Interval upper bound (percentage points).
        ci_high_pp: f64,
    },
    /// The query was refused.
    Refused {
        /// Why the gate refused.
        reason: String,
    },
}

impl GateResponse {
    /// Whether this is a pass.
    pub fn is_pass(&self) -> bool {
        matches!(self, GateResponse::Pass { .. })
    }
}

/// An immutable audit record of one gate query (9.6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// The caller.
    pub caller: CallerIdentity,
    /// Content hash of the candidate being gated.
    pub candidate_hash: String,
    /// EvalContract version in force.
    pub contract_version: u32,
    /// The split queried (always holdout here).
    pub split: Split,
    /// Cumulative queries consumed after this one.
    pub budget_consumed: u32,
    /// The information class returned.
    pub returned: InformationClass,
}

/// The holdout vault: owns holdout tasks and exposes only the gate (EV-14).
///
/// There is deliberately no method to list, fetch or inspect holdout tasks; the
/// firewall is structural.
pub struct HoldoutVault {
    tasks: Vec<Task>,
    contract_version: u32,
    query_budget: u32,
    consumed: u32,
    audit: Vec<AuditEntry>,
    failed_attempts: u32,
}

impl HoldoutVault {
    /// Seal a set of tasks as the holdout under a contract (EL-2: sealed before
    /// optimization). Non-holdout tasks are rejected.
    pub fn seal(contract: &EvalContract, tasks: Vec<Task>) -> Result<Self, VaultError> {
        if let Some(bad) = tasks.iter().find(|t| t.split != Split::Holdout) {
            return Err(VaultError::NotHoldout(bad.id.clone()));
        }
        Ok(Self {
            tasks,
            contract_version: contract.version,
            query_budget: contract.release.holdout_query_budget,
            consumed: 0,
            audit: Vec::new(),
            failed_attempts: 0,
        })
    }

    /// Number of sealed holdout tasks (a count, not their content).
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Whether the vault is empty.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Remaining gate queries.
    pub fn remaining_budget(&self) -> u32 {
        self.query_budget.saturating_sub(self.consumed)
    }

    /// Query the release gate (EV-14). Runs the baseline and candidate over the
    /// sealed holdout internally and returns only a coarse aggregate.
    ///
    /// `caller` must be an assurance identity; design/search is refused (9.6).
    pub fn gate_query(
        &mut self,
        caller: &CallerIdentity,
        candidate_hash: &str,
        contract: &EvalContract,
        baseline: &dyn Fn(&Task) -> bool,
        candidate: &dyn Fn(&Task) -> bool,
    ) -> GateResponse {
        // Firewall: only the assurance plane may query the holdout (9.6, 14.1).
        if caller.plane != Plane::Assurance {
            self.record(caller, candidate_hash, InformationClass::Refused);
            return GateResponse::Refused {
                reason: format!(
                    "plane {:?} may not query the holdout gate (PRD 9.6)",
                    caller.plane
                ),
            };
        }
        // Budget: refuse once exhausted (9.6 reusable-holdout).
        if self.consumed >= self.query_budget {
            self.record(caller, candidate_hash, InformationClass::Refused);
            return GateResponse::Refused {
                reason: "holdout query budget exhausted (PRD EV-14)".into(),
            };
        }

        let base: Vec<bool> = self.tasks.iter().map(baseline).collect();
        let cand: Vec<bool> = self.tasks.iter().map(candidate).collect();
        // Deterministic seed from the candidate hash so repeated queries are stable.
        let seed = seed_from(candidate_hash);
        let cmp = paired_comparison(&base, &cand, seed);

        self.consumed += 1;
        // Calibrated widening grows with budget consumption (9.6).
        let noise_pp = 2.0 * (self.consumed as f64 / self.query_budget.max(1) as f64);
        let ci_low_pp = cmp.ci_low * 100.0 - noise_pp;
        let ci_high_pp = cmp.ci_high * 100.0 + noise_pp;

        self.record(caller, candidate_hash, InformationClass::AggregateGateResult);

        let target = contract.release.target_improvement_pp;
        // Pass when the (noised) lower bound clears zero and the point estimate
        // meets the pre-registered target improvement (PRD 21 exit rule).
        let passes = ci_low_pp > 0.0 && cmp.delta * 100.0 >= target;
        if passes {
            GateResponse::Pass {
                ci_low_pp,
                ci_high_pp,
            }
        } else {
            GateResponse::Fail {
                ci_low_pp,
                ci_high_pp,
            }
        }
    }

    /// Record a failed release attempt (PRD Q12: after two failed attempts every
    /// queried holdout task is retired and >=50% is refreshed). This slice tracks
    /// the counter; refresh sourcing lands with the task-generation pipeline.
    pub fn record_failed_attempt(&mut self) {
        self.failed_attempts += 1;
    }

    /// Whether a holdout refresh is now required (PRD Q12).
    pub fn refresh_required(&self) -> bool {
        self.failed_attempts >= 2
    }

    /// The audit log, readable by auditors (9.6).
    pub fn audit_log(&self, reader: &CallerIdentity) -> Result<&[AuditEntry], VaultError> {
        match reader.plane {
            Plane::Auditor | Plane::Assurance => Ok(&self.audit),
            Plane::DesignSearch => Err(VaultError::Unauthorized),
        }
    }

    fn record(&mut self, caller: &CallerIdentity, candidate_hash: &str, returned: InformationClass) {
        self.audit.push(AuditEntry {
            caller: caller.clone(),
            candidate_hash: candidate_hash.to_string(),
            contract_version: self.contract_version,
            split: Split::Holdout,
            budget_consumed: self.consumed,
            returned,
        });
    }
}

/// Errors from the holdout vault.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VaultError {
    /// A task offered to `seal` was not in the holdout split.
    #[error("task '{0}' is not a holdout task")]
    NotHoldout(String),
    /// The reader may not access this.
    #[error("unauthorized")]
    Unauthorized,
}

fn seed_from(s: &str) -> u64 {
    // FNV-1a over the bytes for a stable, dependency-free seed.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ReleaseRule, SplitPolicy};
    use crate::task::{Difficulty, Provenance};

    fn contract() -> EvalContract {
        EvalContract {
            version: 3,
            criteria: vec!["resolves".into()],
            negative_goals: vec![],
            splits: SplitPolicy::default(),
            release: ReleaseRule {
                primary_metric: "task_success".into(),
                target_improvement_pp: 10.0,
                holdout_query_budget: 2,
            },
        }
    }

    fn holdout_tasks(n: usize) -> Vec<Task> {
        (0..n)
            .map(|i| Task {
                id: format!("h{i}"),
                input: serde_json::json!({ "i": i }),
                environment: "helpdesk".into(),
                checkers: vec!["state".into()],
                tags: vec![],
                difficulty: Difficulty::Medium,
                provenance: Provenance::HumanSeed,
                split: Split::Holdout,
                source_task: None,
            })
            .collect()
    }

    fn assurance() -> CallerIdentity {
        CallerIdentity {
            id: "assure-svc".into(),
            plane: Plane::Assurance,
        }
    }

    #[test]
    fn design_plane_is_refused() {
        let c = contract();
        let mut v = HoldoutVault::seal(&c, holdout_tasks(100)).unwrap();
        let design = CallerIdentity {
            id: "search".into(),
            plane: Plane::DesignSearch,
        };
        let r = v.gate_query(&design, "cand#1", &c, &|_| false, &|_| true);
        assert!(matches!(r, GateResponse::Refused { .. }));
        // The refusal is audited.
        assert_eq!(v.audit_log(&assurance()).unwrap().len(), 1);
    }

    #[test]
    fn clear_improvement_passes_and_consumes_budget() {
        let c = contract();
        let mut v = HoldoutVault::seal(&c, holdout_tasks(100)).unwrap();
        let r = v.gate_query(&assurance(), "cand#1", &c, &|_| false, &|_| true);
        assert!(r.is_pass(), "{r:?}");
        assert_eq!(v.remaining_budget(), 1);
    }

    #[test]
    fn budget_exhaustion_refuses() {
        let c = contract();
        let mut v = HoldoutVault::seal(&c, holdout_tasks(50)).unwrap();
        let _ = v.gate_query(&assurance(), "a", &c, &|_| false, &|_| true);
        let _ = v.gate_query(&assurance(), "b", &c, &|_| false, &|_| true);
        let third = v.gate_query(&assurance(), "c", &c, &|_| false, &|_| true);
        assert!(matches!(third, GateResponse::Refused { .. }));
    }

    #[test]
    fn seal_rejects_non_holdout_tasks() {
        let c = contract();
        let mut tasks = holdout_tasks(1);
        tasks[0].split = Split::Tune;
        assert!(HoldoutVault::seal(&c, tasks).is_err());
    }
}
