//! Architecture Explorer and hierarchical complexity unlocks (PRD 11.3, 11.4, Q4).
//!
//! The engine advances to a higher complexity level only when evidence indicates
//! the lower level cannot meet the EvalContract within budget (PRD 11.4). The
//! seven levels are a default prior, not a strict ladder: each failure class maps
//! to the levels it makes eligible (see `entelechy_failure`), and jumping a level
//! requires the triggering evidence on the DesignHypothesis and counts against the
//! DesignBudget.
//!
//! Unlock policy (Q4): automatic when the failure evidence names a class the next
//! level addresses, the unlock stays within the AuthorityEnvelope and remaining
//! budget, and the GoalSpec complexity ceiling allows it. Anything above the
//! ceiling, or that adds new authority, needs human approval.

use serde::{Deserialize, Serialize};

/// The maximum defined complexity level (PRD 11.4 level 6).
pub const MAX_LEVEL: u8 = 6;

/// A one-line description of a hierarchical search level (PRD 11.4).
pub fn level_description(level: u8) -> &'static str {
    match level {
        0 => "single model call / deterministic wrappers",
        1 => "prompt, model, tool subset and structured-output optimization",
        2 => "retrieval and memory configuration",
        3 => "verification, routing and fallback",
        4 => "parallelism and map/reduce",
        5 => "delegation and multi-agent structure",
        6 => "broader topology exploration",
        _ => "unknown level",
    }
}

/// The decision the Architecture Explorer reaches about unlocking complexity
/// (PRD 11.4, Q4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnlockDecision {
    /// Unlock the target level automatically (within ceiling, budget, authority).
    Automatic {
        /// The level unlocked.
        level: u8,
    },
    /// The unlock is justified by evidence but requires human approval — it is
    /// above the complexity ceiling or would add new authority (Q4).
    NeedsApproval {
        /// The level proposed.
        level: u8,
        /// Why approval is required.
        reason: String,
    },
    /// No unlock: no failure class justifies a higher level, or the budget is
    /// exhausted.
    NoUnlock {
        /// Why no unlock happens.
        reason: String,
    },
}

/// The Architecture Explorer proposes materially different design classes when
/// the current class appears insufficient (PRD 11.3).
pub struct ArchitectureExplorer;

impl ArchitectureExplorer {
    /// Decide whether to unlock a higher complexity level (PRD 11.4, Q4).
    ///
    /// - `current_level`: the current design's complexity level.
    /// - `eligible_levels`: levels made eligible by observed failure classes
    ///   (from `entelechy_failure::FailureClass::eligible_levels`).
    /// - `ceiling`: the GoalSpec complexity ceiling.
    /// - `budget_remaining`: whether the DesignBudget can fund exploration.
    /// - `unlock_adds_authority`: whether the unlock would grant new authority.
    pub fn consider(
        current_level: u8,
        eligible_levels: &[u8],
        ceiling: u8,
        budget_remaining: bool,
        unlock_adds_authority: bool,
    ) -> UnlockDecision {
        // The next level justified by evidence: the smallest eligible level above
        // the current one (levels are a prior, not a strict ladder — a jump is
        // allowed when a class makes a higher level directly eligible).
        let target = eligible_levels
            .iter()
            .copied()
            .filter(|&l| l > current_level && l <= MAX_LEVEL)
            .min();

        let Some(level) = target else {
            return UnlockDecision::NoUnlock {
                reason: "no failure class justifies a higher complexity level".into(),
            };
        };

        if !budget_remaining {
            return UnlockDecision::NoUnlock {
                reason: "design budget exhausted; cannot fund exploration".into(),
            };
        }

        if level > ceiling {
            return UnlockDecision::NeedsApproval {
                level,
                reason: format!("level {level} exceeds the GoalSpec complexity ceiling {ceiling}"),
            };
        }
        if unlock_adds_authority {
            return UnlockDecision::NeedsApproval {
                level,
                reason: "unlock would grant new authority".into(),
            };
        }
        UnlockDecision::Automatic { level }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_unlock_within_ceiling_and_budget() {
        // A verification failure makes level 3 eligible from level 1.
        let d = ArchitectureExplorer::consider(1, &[3], 5, true, false);
        assert_eq!(d, UnlockDecision::Automatic { level: 3 });
    }

    #[test]
    fn level_jump_is_allowed_when_a_class_makes_it_eligible() {
        // Context overflow makes level 4 eligible directly from level 1 (PRD 11.4).
        let d = ArchitectureExplorer::consider(1, &[2, 4], 6, true, false);
        // Smallest eligible above current is 2 here (retrieval), taken first.
        assert_eq!(d, UnlockDecision::Automatic { level: 2 });
        // If only 4 is eligible, it jumps straight to 4.
        let d = ArchitectureExplorer::consider(1, &[4], 6, true, false);
        assert_eq!(d, UnlockDecision::Automatic { level: 4 });
    }

    #[test]
    fn above_ceiling_needs_approval() {
        let d = ArchitectureExplorer::consider(3, &[5], 4, true, false);
        assert!(matches!(d, UnlockDecision::NeedsApproval { level: 5, .. }));
    }

    #[test]
    fn adding_authority_needs_approval() {
        let d = ArchitectureExplorer::consider(1, &[3], 5, true, true);
        assert!(matches!(d, UnlockDecision::NeedsApproval { level: 3, .. }));
    }

    #[test]
    fn no_eligible_level_or_no_budget_is_no_unlock() {
        assert!(matches!(
            ArchitectureExplorer::consider(3, &[1, 2], 6, true, false),
            UnlockDecision::NoUnlock { .. }
        ));
        assert!(matches!(
            ArchitectureExplorer::consider(1, &[3], 6, false, false),
            UnlockDecision::NoUnlock { .. }
        ));
    }
}
