//! Deployment profiles, recovery objectives and evidence state (PRD 17.1, 17.3).
//!
//! An installation declares exactly one profile; guarantees apply only within it
//! (DP-1). Each production topology declares RPO/RTO targets (Q28). After partial
//! loss, affected runs/releases are marked `evidence_incomplete` and cannot be
//! newly promoted until evidence is regenerated or waived (DR-3).

use serde::{Deserialize, Serialize};

/// A supported deployment profile (PRD 17.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeploymentProfile {
    /// Developer / single-user; embedded stores; one security domain.
    Local,
    /// Team or shared service before cluster scale.
    SingleNodeServer,
    /// Multi-worker production with tenant isolation at scale.
    Cluster,
}

/// Recovery objectives for a profile (PRD 17.3, Q28). `rpo_secs` is `None` for
/// local (RPO is the last export); `rto_secs` is `None` where no RTO is claimed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryObjectives {
    /// Recovery point objective in seconds (None = last export, local mode).
    pub rpo_secs: Option<u64>,
    /// Recovery time objective in seconds (None = no RTO claim).
    pub rto_secs: Option<u64>,
}

impl DeploymentProfile {
    /// Whether the profile makes a high-availability claim (PRD 17.1).
    pub fn is_high_availability(self) -> bool {
        matches!(self, DeploymentProfile::Cluster)
    }

    /// The declared RPO/RTO for this profile (PRD Q28).
    pub fn recovery_objectives(self) -> RecoveryObjectives {
        match self {
            // Local: RPO is the last export (auto after each study); no RTO claim.
            DeploymentProfile::Local => RecoveryObjectives {
                rpo_secs: None,
                rto_secs: None,
            },
            // Single-node server: RPO 15 min, RTO 4 h.
            DeploymentProfile::SingleNodeServer => RecoveryObjectives {
                rpo_secs: Some(15 * 60),
                rto_secs: Some(4 * 60 * 60),
            },
            // Cluster: RPO 5 min, RTO 1 h.
            DeploymentProfile::Cluster => RecoveryObjectives {
                rpo_secs: Some(5 * 60),
                rto_secs: Some(60 * 60),
            },
        }
    }

    /// The rollback-time objective for new executions after a rollback (PRD Q24),
    /// in seconds.
    pub fn rollback_objective_secs(self) -> u64 {
        match self {
            DeploymentProfile::Local => 5 * 60,            // 5 minutes
            DeploymentProfile::SingleNodeServer => 2 * 60, // 2 minutes
            DeploymentProfile::Cluster => 60,              // 60 seconds
        }
    }

    /// A one-line guarantee boundary (PRD 17.1).
    pub fn guarantee_boundary(self) -> &'static str {
        match self {
            DeploymentProfile::Local => {
                "no HA claim; export/import and integrity verification required"
            }
            DeploymentProfile::SingleNodeServer => {
                "backups, restore verification, quotas and declared RPO/RTO before L5"
            }
            DeploymentProfile::Cluster => {
                "topology-specific RPO/RTO, failover, tenant isolation and restore drill before L5"
            }
        }
    }

    /// Whether this profile requires a restore drill before reaching L5 (PRD 17.3).
    pub fn requires_restore_drill_for_l5(self) -> bool {
        !matches!(self, DeploymentProfile::Local)
    }
}

/// Whether a run's/release's supporting evidence is complete (PRD 17.3, DR-3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceState {
    /// All required evidence is present.
    Complete,
    /// Evidence was lost (partial loss); new promotion is blocked (DR-3).
    Incomplete,
}

impl EvidenceState {
    /// Whether new promotion is permitted (DR-3: `evidence_incomplete` blocks it).
    pub fn can_promote(self) -> bool {
        matches!(self, EvidenceState::Complete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_declare_recovery_objectives() {
        assert_eq!(
            DeploymentProfile::Local.recovery_objectives(),
            RecoveryObjectives {
                rpo_secs: None,
                rto_secs: None
            }
        );
        assert_eq!(
            DeploymentProfile::SingleNodeServer
                .recovery_objectives()
                .rto_secs,
            Some(4 * 60 * 60)
        );
        assert_eq!(
            DeploymentProfile::Cluster.recovery_objectives().rpo_secs,
            Some(300)
        );
        assert!(DeploymentProfile::Cluster.is_high_availability());
        assert!(!DeploymentProfile::Local.is_high_availability());
    }

    #[test]
    fn rollback_and_drill_requirements() {
        assert_eq!(DeploymentProfile::Cluster.rollback_objective_secs(), 60);
        assert!(!DeploymentProfile::Local.requires_restore_drill_for_l5());
        assert!(DeploymentProfile::SingleNodeServer.requires_restore_drill_for_l5());
    }

    #[test]
    fn evidence_incomplete_blocks_promotion() {
        assert!(EvidenceState::Complete.can_promote());
        assert!(!EvidenceState::Incomplete.can_promote());
    }
}
