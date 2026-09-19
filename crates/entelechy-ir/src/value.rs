//! Value envelope with provenance, taint and confidentiality labels
//! (PRD 7.2, 7.5).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Integrity label: untrusted data must not drive privileged effects (IR-I3).
///
/// Every response from the external capability plane "enters as tainted input"
/// (PRD 5.8, 7.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Taint {
    /// Produced within the trusted control/data plane.
    Trusted,
    /// Derived from external/untrusted content; cannot reach a privileged effect
    /// without an applicable Gate (IR-I3).
    Tainted,
}

impl Taint {
    /// Combine two taints conservatively: any tainted input taints the output
    /// (PRD 7.5 — outputs inherit the union of input labels).
    pub fn join(self, other: Taint) -> Taint {
        match (self, other) {
            (Taint::Trusted, Taint::Trusted) => Taint::Trusted,
            _ => Taint::Tainted,
        }
    }
}

/// A confidentiality label (PRD 7.5). Sensitive data must not leave its scope
/// (IR-I9). Labels propagate as a union unless a Verify/declassification policy
/// lowers them.
pub type Confidentiality = BTreeSet<String>;

/// Verification state of a value (PRD 7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerificationState {
    /// Not yet verified.
    Unverified,
    /// Verified by a Verify node.
    Verified,
    /// Verification attempted and failed.
    Failed,
}

/// Metadata that important values may carry alongside their payload (PRD 7.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValueMeta {
    /// Integrity/taint label.
    pub taint: Taint,
    /// Confidentiality labels (e.g. `pii`, `tenant-private`, `secret-derived`).
    pub confidentiality: Confidentiality,
    /// Provenance references (artifact ids, source uris).
    pub provenance: Vec<String>,
    /// Confidence / uncertainty in `[0.0, 1.0]`, if known.
    pub confidence: Option<f64>,
    /// Node id and run id that produced the value.
    pub producer: Option<String>,
    /// Verification state.
    pub verification: VerificationState,
}

impl Default for ValueMeta {
    fn default() -> Self {
        Self {
            taint: Taint::Trusted,
            confidentiality: BTreeSet::new(),
            provenance: Vec::new(),
            confidence: None,
            producer: None,
            verification: VerificationState::Unverified,
        }
    }
}

impl ValueMeta {
    /// Metadata for freshly ingested external/untrusted content (PRD 5.8).
    pub fn tainted() -> Self {
        Self {
            taint: Taint::Tainted,
            ..Default::default()
        }
    }

    /// Conservative join of two values' metadata (PRD 7.5): taint and
    /// confidentiality labels are unioned; confidence is the minimum.
    pub fn join(&self, other: &ValueMeta) -> ValueMeta {
        let mut confidentiality = self.confidentiality.clone();
        confidentiality.extend(other.confidentiality.iter().cloned());
        let confidence = match (self.confidence, other.confidence) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
        ValueMeta {
            taint: self.taint.join(other.taint),
            confidentiality,
            provenance: {
                let mut p = self.provenance.clone();
                p.extend(other.provenance.iter().cloned());
                p
            },
            confidence,
            producer: None,
            verification: VerificationState::Unverified,
        }
    }
}

/// A value: JSON payload plus its envelope metadata (PRD 7.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Value {
    /// Typed payload.
    pub data: serde_json::Value,
    /// Envelope metadata.
    pub meta: ValueMeta,
}

impl Value {
    /// A trusted value with default metadata.
    pub fn trusted(data: serde_json::Value) -> Self {
        Self {
            data,
            meta: ValueMeta::default(),
        }
    }

    /// A tainted value (external/untrusted content — PRD 5.8).
    pub fn tainted(data: serde_json::Value) -> Self {
        Self {
            data,
            meta: ValueMeta::tainted(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn taint_joins_conservatively() {
        assert_eq!(Taint::Trusted.join(Taint::Trusted), Taint::Trusted);
        assert_eq!(Taint::Trusted.join(Taint::Tainted), Taint::Tainted);
    }

    #[test]
    fn meta_join_unions_labels_and_taint() {
        let mut a = ValueMeta::default();
        a.confidentiality.insert("pii".into());
        let b = ValueMeta::tainted();
        let j = a.join(&b);
        assert_eq!(j.taint, Taint::Tainted);
        assert!(j.confidentiality.contains("pii"));
    }

    #[test]
    fn confidence_join_takes_min() {
        let a = Value::trusted(json!(1));
        let mut m = ValueMeta::default();
        m.confidence = Some(0.9);
        let mut n = ValueMeta::default();
        n.confidence = Some(0.4);
        assert_eq!(m.join(&n).confidence, Some(0.4));
        let _ = a;
    }
}
