//! Model fingerprints and evidence epochs (PRD 5.10, 21.1.1; PV-2).
//!
//! A provider model *name* is not a reproducible identity. A silent provider
//! update — a changed revision behind the same advertised model — is *material
//! drift*: evidence gathered before it does not describe the system after it. PV-2
//! requires that such evidence is never silently pooled.
//!
//! [`ModelFingerprint`] content-addresses the drift-relevant identity fields.
//! [`EvidenceLedger`] keys evidence by fingerprint: a new fingerprint opens a new
//! [`EvidenceEpoch`] instead of extending the old one, and the release decision
//! reads only the current epoch. Pooling across a drift is therefore structurally
//! impossible — the ledger has no method that mixes epochs into release evidence.

use entelechy_gateway::{
    MockModel, ModelError, ModelGateway, ModelRequest, ModelResponse, ProviderIdentity,
};
use serde::{Deserialize, Serialize};

/// A content address over the drift-relevant provider-identity fields (PRD 5.10).
///
/// Two calls share a fingerprint iff their provider, advertised model, endpoint,
/// request-schema version and returned revision all match. A changed revision —
/// the usual shape of a silent provider update — changes the fingerprint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFingerprint(String);

impl ModelFingerprint {
    /// Fingerprint a captured provider identity (PV-1 → PV-2).
    pub fn of(identity: &ProviderIdentity) -> Self {
        // Content-address exactly the identity fields that define material drift
        // (not `sampling`, a caller-chosen request parameter).
        #[derive(Serialize)]
        struct DriftFields<'a> {
            provider: &'a str,
            advertised_model: &'a str,
            endpoint: &'a str,
            request_schema_version: &'a str,
            revision: &'a Option<String>,
        }
        let fields = DriftFields {
            provider: &identity.provider,
            advertised_model: &identity.advertised_model,
            endpoint: &identity.endpoint,
            request_schema_version: &identity.request_schema_version,
            revision: &identity.revision,
        };
        let id = entelechy_artifacts::ArtifactId::of(&fields)
            .map(|i| i.to_string())
            .unwrap_or_default();
        Self(id)
    }

    /// The fingerprint as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Evidence gathered under a single [`ModelFingerprint`] (PRD 21.1.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceEpoch {
    /// The fingerprint this evidence was gathered under.
    pub fingerprint: ModelFingerprint,
    /// One per-task resolution outcome (`true` = resolved).
    pub outcomes: Vec<bool>,
}

impl EvidenceEpoch {
    /// Resolved-task count in this epoch.
    pub fn successes(&self) -> usize {
        self.outcomes.iter().filter(|o| **o).count()
    }

    /// Total observations in this epoch.
    pub fn total(&self) -> usize {
        self.outcomes.len()
    }

    /// Resolved fraction in this epoch (0 for an empty epoch).
    pub fn success_rate(&self) -> f64 {
        if self.outcomes.is_empty() {
            0.0
        } else {
            self.successes() as f64 / self.total() as f64
        }
    }
}

/// An append-only ledger of evidence epochs (PRD 21.1.1, PV-2).
///
/// [`EvidenceLedger::observe`] extends the current epoch only while the fingerprint
/// is unchanged; a new fingerprint opens a new epoch. Release evidence is always a
/// single epoch — the current one — so a material drift resets the evidence that
/// backs a release rather than silently pooling across it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvidenceLedger {
    epochs: Vec<EvidenceEpoch>,
}

impl EvidenceLedger {
    /// Create an empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe outcomes gathered under `fingerprint`. Extends the current epoch
    /// when the fingerprint matches; otherwise opens a new epoch (no pooling across
    /// a material drift).
    pub fn observe(&mut self, fingerprint: &ModelFingerprint, outcomes: &[bool]) {
        match self.epochs.last_mut() {
            Some(epoch) if &epoch.fingerprint == fingerprint => {
                epoch.outcomes.extend_from_slice(outcomes);
            }
            _ => self.epochs.push(EvidenceEpoch {
                fingerprint: fingerprint.clone(),
                outcomes: outcomes.to_vec(),
            }),
        }
    }

    /// All epochs, in observation order.
    pub fn epochs(&self) -> &[EvidenceEpoch] {
        &self.epochs
    }

    /// The current (most recent) epoch, if any.
    pub fn current(&self) -> Option<&EvidenceEpoch> {
        self.epochs.last()
    }

    /// Whether a material drift occurred (more than one epoch exists).
    pub fn drift_detected(&self) -> bool {
        self.epochs.len() > 1
    }

    /// The evidence a release may rely on: the current epoch only (PV-2). After a
    /// drift this is strictly the post-drift evidence — never the pooled history.
    pub fn release_evidence(&self) -> Option<&EvidenceEpoch> {
        self.current()
    }

    /// The naive pooled success count across *all* epochs — the number PV-2
    /// forbids a release from using. Exposed only so a drill can show it differs
    /// from [`EvidenceLedger::release_evidence`].
    pub fn pooled_successes(&self) -> usize {
        self.epochs.iter().map(EvidenceEpoch::successes).sum()
    }

    /// The naive pooled total across all epochs (companion to
    /// [`EvidenceLedger::pooled_successes`]).
    pub fn pooled_total(&self) -> usize {
        self.epochs.iter().map(EvidenceEpoch::total).sum()
    }

    /// Whether pooling across the drift would overstate the current health: the
    /// pooled success rate exceeds the current epoch's. When true, pooling would
    /// mask a post-drift regression — exactly what PV-2 prevents.
    pub fn pooling_would_mask_regression(&self) -> bool {
        let Some(current) = self.current() else {
            return false;
        };
        if !self.drift_detected() || self.pooled_total() == 0 {
            return false;
        }
        let pooled_rate = self.pooled_successes() as f64 / self.pooled_total() as f64;
        pooled_rate > current.success_rate()
    }
}

/// A deterministic test/study gateway that advertises the same model as
/// [`MockModel`] but stamps a chosen provider revision (PRD 5.10).
///
/// It models a provider that ships a new internal revision behind an unchanged
/// advertised model name — the silent material drift PV-2 is about. The reply text
/// is the mock's deterministic derivation, so runs stay reproducible; only the
/// returned `revision` (and hence the [`ModelFingerprint`]) changes.
#[derive(Clone, Debug)]
pub struct RevisionModel {
    revision: String,
}

impl RevisionModel {
    /// A gateway that stamps the given provider revision.
    pub fn new(revision: impl Into<String>) -> Self {
        Self {
            revision: revision.into(),
        }
    }
}

impl ModelGateway for RevisionModel {
    fn infer(&self, req: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let mut resp = MockModel::new().infer(req)?;
        resp.identity.revision = Some(self.revision.clone());
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(revision: &str) -> ProviderIdentity {
        ProviderIdentity {
            provider: "mock".into(),
            advertised_model: "mock-small".into(),
            endpoint: "in-process".into(),
            request_schema_version: "1".into(),
            sampling: serde_json::json!({ "temperature": 0.0 }),
            revision: Some(revision.into()),
        }
    }

    #[test]
    fn fingerprint_changes_with_revision_but_not_with_sampling() {
        let a = ModelFingerprint::of(&identity("mock-r1"));
        let b = ModelFingerprint::of(&identity("mock-r2"));
        assert_ne!(a, b, "a changed revision is material drift");

        // Same identity but different sampling temperature -> same fingerprint.
        let mut hot = identity("mock-r1");
        hot.sampling = serde_json::json!({ "temperature": 0.9 });
        assert_eq!(
            ModelFingerprint::of(&identity("mock-r1")),
            ModelFingerprint::of(&hot),
            "sampling is a caller parameter, not provider drift"
        );
    }

    #[test]
    fn a_new_fingerprint_opens_a_new_epoch_never_pooling() {
        let mut ledger = EvidenceLedger::new();
        let r1 = ModelFingerprint::of(&identity("mock-r1"));
        let r2 = ModelFingerprint::of(&identity("mock-r2"));

        // Strong pre-drift evidence under r1.
        ledger.observe(&r1, &[true, true, true, true]);
        // Same fingerprint extends the same epoch.
        ledger.observe(&r1, &[true]);
        assert_eq!(ledger.epochs().len(), 1);
        assert_eq!(ledger.current().unwrap().successes(), 5);

        // Drift to r2 with a regression (the drifted model fails to ground).
        ledger.observe(&r2, &[false, false]);
        assert!(ledger.drift_detected());
        assert_eq!(ledger.epochs().len(), 2, "a new epoch, not a pooled one");

        // Release evidence is the current epoch only: 0/2, not the pooled 5/7.
        let release = ledger.release_evidence().unwrap();
        assert_eq!(release.successes(), 0);
        assert_eq!(release.total(), 2);
        assert_eq!(ledger.pooled_successes(), 5);
        assert_eq!(ledger.pooled_total(), 7);
        assert!(
            ledger.pooling_would_mask_regression(),
            "pooling 5/7 would hide the post-drift 0/2"
        );
    }

    #[test]
    fn revision_model_only_changes_the_revision() {
        let a = RevisionModel::new("mock-r1");
        let b = RevisionModel::new("mock-r2");
        let req = ModelRequest {
            model: "mock-small".into(),
            prompt: "resolve".into(),
            temperature: 0.0,
        };
        let ra = a.infer(&req).unwrap();
        let rb = b.infer(&req).unwrap();
        // Same deterministic text, different revision.
        assert_eq!(ra.text, rb.text);
        assert_eq!(ra.identity.revision.as_deref(), Some("mock-r1"));
        assert_eq!(rb.identity.revision.as_deref(), Some("mock-r2"));
        assert_ne!(
            ModelFingerprint::of(&ra.identity),
            ModelFingerprint::of(&rb.identity)
        );
    }

    #[test]
    fn no_drift_means_no_masking() {
        let mut ledger = EvidenceLedger::new();
        let r1 = ModelFingerprint::of(&identity("mock-r1"));
        ledger.observe(&r1, &[true, false, true]);
        assert!(!ledger.drift_detected());
        assert!(!ledger.pooling_would_mask_regression());
        assert_eq!(ledger.release_evidence().unwrap().total(), 3);
    }
}
