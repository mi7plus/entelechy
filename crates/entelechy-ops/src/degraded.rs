//! Degraded operation and independently-authorized fallbacks (PRD 17.4, OL-2).
//!
//! When a primary path is unavailable the system may fall back — to a secondary
//! model, provider or tool — but a fallback never inherits the primary's authority.
//! OL-2 requires three things:
//!
//! 1. **Fallbacks are independently authorized** ([`authorize_fallback`]): the
//!    fallback is checked against the required capabilities and confidentiality tier
//!    on its *own* grants; a fallback that lacks a required capability, or would
//!    weaken confidentiality, is refused — regardless of what the primary held.
//! 2. **Degraded mode is journaled** ([`DegradedJournal`]): entering and leaving
//!    degraded operation is recorded in an append-only journal with a reason, so the
//!    period of reduced guarantees is auditable.
//! 3. **Degraded-mode results are excluded from registered evidence**
//!    ([`registered_evidence`]): observations gathered while degraded are tagged and
//!    never counted as the registered evidence a release decision rests on.

use serde::{Deserialize, Serialize};

use std::collections::BTreeSet;

/// A confidentiality tier (PRD 7.5). Ordered from least to most restricted, so a
/// fallback must sit at or above the required tier to avoid weakening it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfidentialityTier {
    /// Freely shareable.
    Public,
    /// Internal use.
    Internal,
    /// Restricted / sensitive.
    Restricted,
}

/// What a path (primary or fallback) is authorized to do (PRD 8.3, 17.4): the
/// capabilities it holds and the confidentiality tier it operates at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authorization {
    /// Capabilities this path independently holds.
    pub capabilities: BTreeSet<String>,
    /// The confidentiality tier this path can operate at.
    pub confidentiality: ConfidentialityTier,
}

impl Authorization {
    /// Build an authorization from capability names and a confidentiality tier.
    pub fn new(
        capabilities: impl IntoIterator<Item = String>,
        confidentiality: ConfidentialityTier,
    ) -> Self {
        Self {
            capabilities: capabilities.into_iter().collect(),
            confidentiality,
        }
    }
}

/// Why a fallback was refused (OL-2 — a fallback is independently authorized or it
/// does not run).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FallbackRefused {
    /// The fallback does not independently hold a required capability.
    #[error("fallback lacks required capability '{0}' (not inherited from the primary)")]
    MissingCapability(String),
    /// The fallback would operate at a weaker confidentiality tier than required.
    #[error("fallback would weaken confidentiality from {required:?} to {fallback:?}")]
    WeakerConfidentiality {
        /// The required tier.
        required: ConfidentialityTier,
        /// The fallback's (weaker) tier.
        fallback: ConfidentialityTier,
    },
}

/// Authorize a fallback independently of the primary (OL-2). The fallback runs only
/// if it *itself* holds every required capability and operates at or above the
/// required confidentiality tier; it never inherits the primary's authority.
pub fn authorize_fallback(
    required: &Authorization,
    fallback: &Authorization,
) -> Result<(), FallbackRefused> {
    for cap in &required.capabilities {
        if !fallback.capabilities.contains(cap) {
            return Err(FallbackRefused::MissingCapability(cap.clone()));
        }
    }
    if fallback.confidentiality < required.confidentiality {
        return Err(FallbackRefused::WeakerConfidentiality {
            required: required.confidentiality,
            fallback: fallback.confidentiality,
        });
    }
    Ok(())
}

/// One append-only degraded-mode journal entry (OL-2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DegradedEntry {
    /// Timestamp (unix seconds).
    pub at: u64,
    /// `true` on entering degraded mode, `false` on leaving it.
    pub entering: bool,
    /// Why degraded mode was entered/left (e.g. "primary model unavailable").
    pub reason: String,
}

/// An append-only journal of degraded-mode transitions (OL-2). The period of
/// reduced guarantees is auditable: nothing enters or leaves degraded mode without
/// a recorded reason.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DegradedJournal {
    entries: Vec<DegradedEntry>,
    degraded: bool,
}

impl DegradedJournal {
    /// Create an empty journal (normal operation).
    pub fn new() -> Self {
        Self::default()
    }

    /// Record entering degraded mode with a reason.
    pub fn enter(&mut self, at: u64, reason: impl Into<String>) {
        self.degraded = true;
        self.entries.push(DegradedEntry {
            at,
            entering: true,
            reason: reason.into(),
        });
    }

    /// Record leaving degraded mode (recovery to normal operation).
    pub fn exit(&mut self, at: u64, reason: impl Into<String>) {
        self.degraded = false;
        self.entries.push(DegradedEntry {
            at,
            entering: false,
            reason: reason.into(),
        });
    }

    /// Whether the system is currently in degraded mode.
    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    /// The recorded transitions.
    pub fn entries(&self) -> &[DegradedEntry] {
        &self.entries
    }

    /// Number of recorded transitions.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the journal is empty (nothing has ever gone degraded).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One online observation of a run's outcome (PRD OP-2, 17.4). `degraded` marks
/// observations gathered while the system was in degraded mode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    /// The run observed.
    pub run_id: String,
    /// Whether the outcome was a success.
    pub success: bool,
    /// Whether the system was in degraded mode when this was gathered.
    pub degraded: bool,
}

/// The registered evidence a release decision may rest on (OL-2): observations
/// gathered in normal operation. Degraded-mode observations are *excluded* — they
/// stay as unregistered operational signal and never silently back a release.
pub fn registered_evidence(observations: &[Observation]) -> Vec<&Observation> {
    observations.iter().filter(|o| !o.degraded).collect()
}

/// Count of degraded-mode observations excluded from registered evidence (OL-2).
pub fn excluded_from_registered(observations: &[Observation]) -> usize {
    observations.iter().filter(|o| o.degraded).count()
}

/// The result of the OL-2 degraded-mode drill — one boolean per invariant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DegradedModeDrillReport {
    /// An independently-authorized fallback is admitted.
    pub fallback_authorized_when_independent: bool,
    /// A fallback missing a required capability is refused (not inherited).
    pub fallback_refused_when_missing_capability: bool,
    /// A fallback that would weaken confidentiality is refused.
    pub fallback_refused_when_weaker_confidentiality: bool,
    /// Entering degraded mode is journaled with a reason.
    pub degraded_mode_journaled: bool,
    /// Degraded-mode observations are excluded from registered evidence.
    pub degraded_evidence_excluded: bool,
    /// Observations excluded from registered evidence (degraded).
    pub excluded_count: usize,
    /// Observations admitted as registered evidence (normal).
    pub registered_count: usize,
    /// Whether every OL-2 invariant held.
    pub ok: bool,
}

/// Run the OL-2 degraded-mode drill (PRD 17.4): independently-authorized fallbacks,
/// a journaled degraded period, and degraded evidence excluded from the registered
/// set.
pub fn degraded_mode_drill() -> DegradedModeDrillReport {
    // A privileged primary path: it may call the model and read restricted data.
    let required = Authorization::new(
        ["model_infer".to_string(), "read_kb".to_string()],
        ConfidentialityTier::Restricted,
    );

    // An independently-authorized fallback holds the same capabilities at the same
    // tier — admitted on its own authority, not the primary's.
    let good_fallback = Authorization::new(
        ["model_infer".to_string(), "read_kb".to_string()],
        ConfidentialityTier::Restricted,
    );
    let fallback_authorized_when_independent =
        authorize_fallback(&required, &good_fallback).is_ok();

    // A fallback missing a capability is refused (it does not inherit `read_kb`).
    let under_capable =
        Authorization::new(["model_infer".to_string()], ConfidentialityTier::Restricted);
    let fallback_refused_when_missing_capability = matches!(
        authorize_fallback(&required, &under_capable),
        Err(FallbackRefused::MissingCapability(_))
    );

    // A fallback that would operate at a weaker confidentiality tier is refused.
    let leaky = Authorization::new(
        ["model_infer".to_string(), "read_kb".to_string()],
        ConfidentialityTier::Public,
    );
    let fallback_refused_when_weaker_confidentiality = matches!(
        authorize_fallback(&required, &leaky),
        Err(FallbackRefused::WeakerConfidentiality { .. })
    );

    // Entering degraded mode on fallback is journaled with a reason.
    let mut journal = DegradedJournal::new();
    journal.enter(1_000, "primary model unavailable; failing over to fallback");
    let degraded_mode_journaled = journal.is_degraded()
        && journal.len() == 1
        && journal.entries()[0].entering
        && !journal.entries()[0].reason.is_empty();

    // Observations gathered while degraded are excluded from registered evidence.
    let observations = vec![
        Observation {
            run_id: "r1".into(),
            success: true,
            degraded: false,
        },
        Observation {
            run_id: "r2".into(),
            success: true,
            degraded: true, // gathered on the fallback, in degraded mode
        },
        Observation {
            run_id: "r3".into(),
            success: false,
            degraded: true,
        },
        Observation {
            run_id: "r4".into(),
            success: true,
            degraded: false,
        },
    ];
    let registered = registered_evidence(&observations);
    let excluded_count = excluded_from_registered(&observations);
    let registered_count = registered.len();
    let degraded_evidence_excluded =
        registered.iter().all(|o| !o.degraded) && excluded_count == 2 && registered_count == 2;

    let ok = fallback_authorized_when_independent
        && fallback_refused_when_missing_capability
        && fallback_refused_when_weaker_confidentiality
        && degraded_mode_journaled
        && degraded_evidence_excluded;

    DegradedModeDrillReport {
        fallback_authorized_when_independent,
        fallback_refused_when_missing_capability,
        fallback_refused_when_weaker_confidentiality,
        degraded_mode_journaled,
        degraded_evidence_excluded,
        excluded_count,
        registered_count,
        ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degraded_mode_drill_satisfies_ol2() {
        let r = degraded_mode_drill();
        assert!(r.fallback_authorized_when_independent);
        assert!(r.fallback_refused_when_missing_capability);
        assert!(r.fallback_refused_when_weaker_confidentiality);
        assert!(r.degraded_mode_journaled);
        assert!(r.degraded_evidence_excluded);
        assert_eq!(r.excluded_count, 2);
        assert_eq!(r.registered_count, 2);
        assert!(r.ok);
    }

    #[test]
    fn fallback_does_not_inherit_primary_authority() {
        let required =
            Authorization::new(["send_email".to_string()], ConfidentialityTier::Internal);
        // A fallback with no capabilities is refused even though a primary with the
        // capability triggered the fallback — authority is not inherited.
        let empty = Authorization::new(Vec::<String>::new(), ConfidentialityTier::Internal);
        assert_eq!(
            authorize_fallback(&required, &empty),
            Err(FallbackRefused::MissingCapability("send_email".into()))
        );
        // The same capability, held independently, is admitted.
        let independent =
            Authorization::new(["send_email".to_string()], ConfidentialityTier::Internal);
        assert!(authorize_fallback(&required, &independent).is_ok());
    }

    #[test]
    fn stronger_confidentiality_fallback_is_allowed() {
        // A fallback at a stricter tier than required does not weaken confidentiality.
        let required = Authorization::new(["x".to_string()], ConfidentialityTier::Internal);
        let stricter = Authorization::new(["x".to_string()], ConfidentialityTier::Restricted);
        assert!(authorize_fallback(&required, &stricter).is_ok());
    }

    #[test]
    fn degraded_observations_never_register() {
        let obs = vec![
            Observation {
                run_id: "a".into(),
                success: true,
                degraded: true,
            },
            Observation {
                run_id: "b".into(),
                success: true,
                degraded: false,
            },
        ];
        let reg = registered_evidence(&obs);
        assert_eq!(reg.len(), 1);
        assert_eq!(reg[0].run_id, "b");
        assert_eq!(excluded_from_registered(&obs), 1);
    }

    #[test]
    fn journal_records_enter_and_exit() {
        let mut j = DegradedJournal::new();
        assert!(!j.is_degraded());
        j.enter(10, "primary down");
        assert!(j.is_degraded());
        j.exit(20, "primary recovered");
        assert!(!j.is_degraded());
        assert_eq!(j.len(), 2);
        assert!(j.entries()[0].entering);
        assert!(!j.entries()[1].entering);
    }
}
