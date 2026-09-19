//! Model gateway: provider abstraction and identity capture (PRD 8.3, 5.10).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A request to a model endpoint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    /// Logical model identity requested.
    pub model: String,
    /// Full prompt text.
    pub prompt: String,
    /// Sampling temperature.
    pub temperature: f64,
}

/// Recorded provider identity for one model call (PRD 5.10, PV-1).
///
/// A provider model *name* is not a reproducible identity, so every call records
/// the fields needed to detect drift and define evidence epochs (PRD 5.10).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderIdentity {
    /// Provider (e.g. `mock`, `openai-compatible`).
    pub provider: String,
    /// Advertised model / version string.
    pub advertised_model: String,
    /// Endpoint / region.
    pub endpoint: String,
    /// Request schema version.
    pub request_schema_version: String,
    /// Sampling parameters echoed back (temperature, etc.).
    pub sampling: serde_json::Value,
    /// Provider-returned revision / fingerprint field, when available.
    pub revision: Option<String>,
}

/// A model response plus the provider identity captured for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelResponse {
    /// Generated text.
    pub text: String,
    /// Identity captured for this call (PV-1).
    pub identity: ProviderIdentity,
}

/// Errors from a model gateway.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// The requested model is not configured.
    #[error("unknown model: {0}")]
    UnknownModel(String),
    /// Transport or provider failure.
    #[error("provider error: {0}")]
    Provider(String),
}

/// Provider abstraction (PRD 8.3). Routing, retries, fallbacks, budgets and
/// caching layer on top of this trait; a fallback model must independently
/// satisfy authority and confidentiality constraints (PRD 17.4).
pub trait ModelGateway {
    /// Perform inference, capturing provider identity (PV-1).
    fn infer(&self, req: &ModelRequest) -> Result<ModelResponse, ModelError>;
}

/// A deterministic mock model for tests, replay and the 30-minute quickstart
/// (PRD Q7 local runtime, RK deterministic mock harness).
///
/// The response is a pure function of the request, so recorded-effect replay is
/// exact (PRD 18 replay determinism).
#[derive(Clone, Debug, Default)]
pub struct MockModel;

impl MockModel {
    /// Create a mock model.
    pub fn new() -> Self {
        Self
    }

    /// The deterministic derivation used for both live and replay paths.
    fn derive(req: &ModelRequest) -> String {
        let mut hasher = Sha256::new();
        hasher.update(req.model.as_bytes());
        hasher.update([0u8]);
        hasher.update(req.prompt.as_bytes());
        let digest = hex::encode(hasher.finalize());
        format!("mock:{}", &digest[..16])
    }
}

impl ModelGateway for MockModel {
    fn infer(&self, req: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let text = Self::derive(req);
        Ok(ModelResponse {
            text,
            identity: ProviderIdentity {
                provider: "mock".into(),
                advertised_model: req.model.clone(),
                endpoint: "in-process".into(),
                request_schema_version: "1".into(),
                sampling: serde_json::json!({ "temperature": req.temperature }),
                // Deterministic build: a fixed revision so fingerprints are stable.
                revision: Some("mock-r1".into()),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_is_deterministic() {
        let g = MockModel::new();
        let req = ModelRequest {
            model: "mock".into(),
            prompt: "hello".into(),
            temperature: 0.0,
        };
        let a = g.infer(&req).unwrap();
        let b = g.infer(&req).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.identity.provider, "mock");
    }

    #[test]
    fn different_prompts_differ() {
        let g = MockModel::new();
        let mk = |p: &str| ModelRequest {
            model: "mock".into(),
            prompt: p.into(),
            temperature: 0.0,
        };
        assert_ne!(g.infer(&mk("a")).unwrap().text, g.infer(&mk("b")).unwrap().text);
    }
}
